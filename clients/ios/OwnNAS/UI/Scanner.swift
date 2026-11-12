import CoreText
import SwiftUI
import UniformTypeIdentifiers
import Vision
import VisionKit

/// Pages from the document camera, waiting for a name and a format.
struct ScannedPages: Identifiable {
    let id = UUID()
    let pages: [UIImage]
    let taken = Date()
}

/// The system document camera: it finds the page's edges and takes the photo when the page is
/// steady and in focus, straightens it, and offers more pages, retakes, cropping, rotating, and
/// filters (colour, greyscale, black and white, photo).
struct DocumentScanner: UIViewControllerRepresentable {
    let onFinish: ([UIImage]) -> Void

    /// False on devices without a camera. Debug builds on the Simulator make sample pages
    /// instead of opening the camera, so saving and uploading can be tried there.
    static var available: Bool {
        #if DEBUG && targetEnvironment(simulator)
        return true
        #else
        return VNDocumentCameraViewController.isSupported
        #endif
    }

    func makeUIViewController(context: Context) -> UIViewController {
        #if DEBUG && targetEnvironment(simulator)
        // The Simulator shows the document camera but has no camera to take a page with.
        let controller = UIViewController()
        DispatchQueue.main.async { onFinish(samplePages()) }
        return controller
        #else
        let camera = VNDocumentCameraViewController()
        camera.delegate = context.coordinator
        return camera
        #endif
    }

    func updateUIViewController(_ controller: UIViewController, context: Context) {}

    func makeCoordinator() -> Coordinator { Coordinator(onFinish: onFinish) }

    final class Coordinator: NSObject, VNDocumentCameraViewControllerDelegate {
        let onFinish: ([UIImage]) -> Void
        init(onFinish: @escaping ([UIImage]) -> Void) { self.onFinish = onFinish }

        func documentCameraViewController(_ controller: VNDocumentCameraViewController, didFinishWith scan: VNDocumentCameraScan) {
            onFinish((0..<scan.pageCount).map(scan.imageOfPage(at:)))
        }

        func documentCameraViewControllerDidCancel(_ controller: VNDocumentCameraViewController) { onFinish([]) }

        func documentCameraViewController(_ controller: VNDocumentCameraViewController, didFailWithError error: Error) { onFinish([]) }
    }
}

#if DEBUG && targetEnvironment(simulator)
/// Two printed-looking pages, for trying the scanner's save and upload on the Simulator.
private func samplePages() -> [UIImage] {
    let lines = [
        ["OwnNAS scanner test", "Invoice number 4711", "Total due: 128.40 EUR", "Thank you for your business."],
        ["Second page", "The quick brown fox jumps over the lazy dog.", "Searchable text in scanned documents."],
    ]
    return lines.map { page in
        UIGraphicsImageRenderer(size: CGSize(width: 1240, height: 1754)).image { context in
            UIColor.white.setFill()
            context.fill(CGRect(x: 0, y: 0, width: 1240, height: 1754))
            for (index, line) in page.enumerated() {
                (line as NSString).draw(at: CGPoint(x: 120, y: 160 + index * 110), withAttributes: [
                    .font: UIFont.systemFont(ofSize: index == 0 ? 64 : 44, weight: index == 0 ? .bold : .regular),
                    .foregroundColor: UIColor.black,
                ])
            }
        }
    }
}
#endif

enum ScanFormat: Hashable { case pdf, jpeg }

/// Names a scan and chooses one PDF (with the text read from the pages, so it can be searched
/// and selected) or a JPEG per page, then hands the files to the uploader.
struct ScanSheet: View {
    let scan: ScannedPages
    let upload: ([Uploader.Pending]) -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var name: String
    @State private var format = ScanFormat.pdf
    @State private var searchable = true
    @State private var working = false
    @State private var problem: String?

    init(scan: ScannedPages, upload: @escaping ([Uploader.Pending]) -> Void) {
        self.scan = scan
        self.upload = upload
        _name = State(initialValue: "Scan " + scan.taken.formatted(.verbatim("\(year: .defaultDigits)-\(month: .twoDigits)-\(day: .twoDigits) \(hour: .twoDigits(clock: .twentyFourHour, hourCycle: .zeroBased)).\(minute: .twoDigits)", timeZone: .current, calendar: .current)))
    }

    private var trimmed: String { name.trimmingCharacters(in: .whitespacesAndNewlines) }
    private var valid: Bool { !trimmed.isEmpty && trimmed != "." && trimmed != ".." && !trimmed.contains("/") && !trimmed.contains("\\") }
    private var count: Int { scan.pages.count }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    ScrollView(.horizontal, showsIndicators: false) {
                        HStack(spacing: 10) {
                            ForEach(scan.pages.indices, id: \.self) { index in
                                Image(uiImage: scan.pages[index])
                                    .resizable()
                                    .scaledToFit()
                                    .frame(height: 140)
                                    .overlay(RoundedRectangle(cornerRadius: 4).stroke(.separator))
                            }
                        }
                        .padding(.vertical, 4)
                    }
                }
                Section {
                    TextField("Name", text: $name)
                        .autocorrectionDisabled()
                } footer: {
                    Text(valid || trimmed.isEmpty ? (count == 1 ? "1 page" : "\(count) pages") : "Names cannot contain / or \\")
                }
                Section {
                    Picker("Save as", selection: $format) {
                        Text("PDF").tag(ScanFormat.pdf)
                        Text(count == 1 ? "JPEG" : "\(count) JPEGs").tag(ScanFormat.jpeg)
                    }
                    .pickerStyle(.segmented)
                    if format == .pdf {
                        Toggle("Recognize text", isOn: $searchable)
                    }
                } footer: {
                    if format == .pdf && searchable {
                        Text("Reads the words on each page into the PDF, so it can be searched and its text selected and copied.")
                    }
                }
            }
            .disabled(working)
            .navigationTitle("Save scan")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Discard", role: .destructive) { dismiss() }.disabled(working)
                }
                ToolbarItem(placement: .confirmationAction) {
                    if working { ProgressView() } else { Button("Upload") { save() }.disabled(!valid) }
                }
            }
            .alert(problem ?? "", isPresented: Binding(get: { problem != nil }, set: { if !$0 { problem = nil } })) {
                Button("OK", role: .cancel) {}
            }
        }
        .interactiveDismissDisabled(working)
    }

    private func save() {
        working = true
        let pages = scan.pages, name = trimmed, format = format, searchable = searchable, taken = scan.taken
        Task {
            do {
                let files = try await Task.detached(priority: .userInitiated) {
                    try ScanWriter.write(pages, name: name, format: format, recognizeText: searchable, taken: taken)
                }.value
                upload(files)
                dismiss()
            } catch {
                problem = userMessage(error)
                working = false
            }
        }
    }
}

/// Turns scanned pages into files in the app's temporary folder.
enum ScanWriter {
    /// About 300 dots per inch on an A4 or Letter page.
    private static let longestSide: CGFloat = 3300

    static func write(_ pages: [UIImage], name: String, format: ScanFormat, recognizeText: Bool, taken: Date) throws -> [Uploader.Pending] {
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent("uploads/\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        var files: [URL] = []
        switch format {
        case .pdf:
            let url = folder.appendingPathComponent("\(name).pdf")
            try writePDF(pages, to: url, title: name, recognizeText: recognizeText)
            files = [url]
        case .jpeg:
            let digits = String(pages.count).count
            for (index, page) in pages.enumerated() {
                let number = String(repeating: "0", count: digits - String(index + 1).count) + String(index + 1)
                let url = folder.appendingPathComponent(pages.count == 1 ? "\(name).jpg" : "\(name) \(number).jpg")
                guard let data = jpeg(page, quality: 0.85) else { throw ClientError.Failed(reason: "The scan could not be saved") }
                try data.write(to: url)
                files.append(url)
            }
        }
        for url in files { try? FileManager.default.setAttributes([.modificationDate: taken], ofItemAtPath: url.path) }
        return files.map { Uploader.Pending(name: $0.lastPathComponent, file: $0, temporary: true) }
    }

    /// One page per scan, as wide as an A4 page (Letter in the US) and as tall as the scan's
    /// shape needs. The JPEG goes into the PDF as it is, and the words Vision reads are written
    /// over it as invisible text, as scanners do for "searchable PDFs".
    static func writePDF(_ pages: [UIImage], to url: URL, title: String, recognizeText: Bool) throws {
        let info: [CFString: Any] = [kCGPDFContextTitle: title, kCGPDFContextCreator: "OwnNAS"]
        guard let pdf = CGContext(url as CFURL, mediaBox: nil, info as CFDictionary) else {
            throw ClientError.Failed(reason: "The PDF could not be made")
        }
        let width: CGFloat = Locale.current.measurementSystem == .us ? 612 : 595.28
        for page in pages {
            guard let data = jpeg(page, quality: 0.75),
                  let provider = CGDataProvider(data: data as CFData),
                  let image = CGImage(jpegDataProviderSource: provider, decode: nil, shouldInterpolate: true, intent: .defaultIntent)
            else { throw ClientError.Failed(reason: "The scan could not be saved") }
            let height = (width * CGFloat(image.height) / CGFloat(image.width)).rounded()
            var box = CGRect(x: 0, y: 0, width: width, height: height)
            pdf.beginPage(mediaBox: &box)
            pdf.draw(image, in: box)
            if recognizeText { drawInvisibleText(read(image), on: pdf, size: box.size) }
            pdf.endPage()
        }
        pdf.closePDF()
    }

    /// The lines of text Vision finds, with their boxes (0 to 1, from the bottom left).
    private static func read(_ image: CGImage) -> [(String, CGRect)] {
        let request = VNRecognizeTextRequest()
        request.recognitionLevel = .accurate
        request.usesLanguageCorrection = true
        request.automaticallyDetectsLanguage = true
        do {
            try VNImageRequestHandler(cgImage: image).perform([request])
        } catch {
            return []
        }
        return (request.results ?? []).compactMap { observation in
            guard let text = observation.topCandidates(1).first?.string, !text.isEmpty else { return nil }
            return (text, observation.boundingBox)
        }
    }

    private static func drawInvisibleText(_ lines: [(String, CGRect)], on pdf: CGContext, size: CGSize) {
        pdf.saveGState()
        pdf.setTextDrawingMode(.invisible)
        for (text, normalized) in lines {
            let rect = CGRect(x: normalized.minX * size.width, y: normalized.minY * size.height, width: normalized.width * size.width, height: normalized.height * size.height)
            guard rect.width > 0, rect.height > 0 else { continue }
            let font = CTFontCreateWithName("Helvetica" as CFString, rect.height * 0.85, nil)
            let line = CTLineCreateWithAttributedString(NSAttributedString(string: text, attributes: [.font: font]))
            var ascent: CGFloat = 0, descent: CGFloat = 0, leading: CGFloat = 0
            let measured = CGFloat(CTLineGetTypographicBounds(line, &ascent, &descent, &leading))
            guard measured > 0 else { continue }
            pdf.saveGState()
            // Stretch the line to the width Vision found, so a selection covers the printed words.
            pdf.translateBy(x: rect.minX, y: rect.minY + descent)
            pdf.scaleBy(x: rect.width / measured, y: 1)
            pdf.textMatrix = .identity
            pdf.textPosition = .zero
            CTLineDraw(line, pdf)
            pdf.restoreGState()
        }
        pdf.restoreGState()
    }

    /// The page as a JPEG, upright and no bigger than about 300 dots per inch on paper.
    private static func jpeg(_ page: UIImage, quality: CGFloat) -> Data? {
        let longest = max(page.size.width * page.scale, page.size.height * page.scale)
        let factor = min(1, longestSide / longest)
        let target = CGSize(width: (page.size.width * page.scale * factor).rounded(), height: (page.size.height * page.scale * factor).rounded())
        let format = UIGraphicsImageRendererFormat()
        format.scale = 1
        format.opaque = true
        let upright = UIGraphicsImageRenderer(size: target, format: format).image { _ in
            page.draw(in: CGRect(origin: .zero, size: target))
        }
        return upright.jpegData(compressionQuality: quality)
    }
}
