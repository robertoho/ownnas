import SwiftUI
import UIKit

/// One terminal, drawn from the Rust emulator's screen, with the web terminal's extra keys.
struct TerminalScreenView: View {
    @EnvironmentObject private var hub: Hub
    let server: String
    let id: String
    @StateObject private var model = TerminalModel()

    var body: some View {
        TerminalCanvas(model: model)
            .ignoresSafeArea(.container, edges: .bottom)
            .navigationTitle(model.title ?? "Terminal")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .principal) {
                    VStack(spacing: 0) {
                        Text(model.title ?? "Terminal").font(.headline).lineLimit(1)
                        if let state = model.stateText { Text(state).font(.caption2).foregroundStyle(.secondary) }
                    }
                }
                ToolbarItem(placement: .primaryAction) {
                    Menu {
                        Button("Paste", systemImage: "doc.on.clipboard") { model.pasteClipboard() }
                        Button("Smaller text", systemImage: "textformat.size.smaller") { model.fontSize = max(8, model.fontSize - 1) }
                        Button("Larger text", systemImage: "textformat.size.larger") { model.fontSize = min(24, model.fontSize + 1) }
                        Button("Hide keyboard", systemImage: "keyboard.chevron.compact.down") { model.view?.resignFirstResponder() }
                    } label: { Image(systemName: "ellipsis.circle") }
                }
            }
            .task {
                await model.connect(hub, server: server, id: id)
            }
            .onDisappear { model.disconnect() }
    }
}

/// The connection to one terminal; redraws when the screen changes.
@MainActor
final class TerminalModel: ObservableObject {
    @Published var title: String?
    @Published var stateText: String? = "Connecting…"
    @Published var fontSize: CGFloat = 13 { didSet { view?.fontChanged() } }
    private(set) var session: TerminalSession?
    weak var view: TerminalUIView?
    private var listener: Listener?

    func connect(_ hub: Hub, server: String, id: String) async {
        guard session == nil else { return }
        let listener = Listener(model: self)
        self.listener = listener
        let (rows, cols) = view?.gridSize() ?? (40, 80)
        do {
            session = try await hub.call(server) { try $0.connectTerminal(id: id, rows: rows, cols: cols, listener: listener) }
            _ = try? await hub.call(server) { try $0.markTerminalSeen(id: id) }
            screenChanged()
            view?.becomeFirstResponder()
        } catch {
            stateText = userMessage(error)
        }
    }

    func disconnect() {
        session?.disconnect()
        session = nil
    }

    func screenChanged() {
        guard let session else { return }
        let screen = session.snapshot()
        if let newTitle = screen.title, newTitle != title { title = newTitle }
        view?.show(screen)
    }

    func connectionChanged(_ connection: TerminalConnection, _ message: String?) {
        switch connection {
        case .connecting: stateText = "Connecting…"
        case .live: stateText = nil
        case .reconnecting: stateText = message ?? "Reconnecting…"
        case let .exited(code): stateText = (code ?? 0) == 0 ? "Ended" : "Ended · exit code \(code!)"
        case .closed: stateText = "Closed"
        }
    }

    func pasteClipboard() {
        if let text = UIPasteboard.general.string { session?.paste(text: text) }
    }

    /// Hears the session on its own thread and hands over to the main thread, at most once a frame.
    final class Listener: TerminalListener, @unchecked Sendable {
        private weak var model: TerminalModel?
        private let lock = NSLock()
        private var pending = false

        init(model: TerminalModel) { self.model = model }

        func screenChanged() {
            lock.lock()
            let schedule = !pending
            pending = true
            lock.unlock()
            guard schedule else { return }
            DispatchQueue.main.asyncAfter(deadline: .now() + 1.0 / 60) { [weak self] in
                guard let self else { return }
                self.lock.lock()
                self.pending = false
                self.lock.unlock()
                MainActor.assumeIsolated { self.model?.screenChanged() }
            }
        }

        func connectionChanged(connection: TerminalConnection, message: String?) {
            DispatchQueue.main.async { [weak self] in
                MainActor.assumeIsolated { self?.model?.connectionChanged(connection, message) }
            }
        }
    }
}

private struct TerminalCanvas: UIViewRepresentable {
    @ObservedObject var model: TerminalModel

    func makeUIView(context: Context) -> TerminalUIView {
        let view = TerminalUIView(model: model)
        model.view = view
        return view
    }

    func updateUIView(_ view: TerminalUIView, context: Context) {}
}

/// Draws the terminal's cells and takes the keyboard: every key goes straight to the shell.
final class TerminalUIView: UIView, UIKeyInput {
    private weak var model: TerminalModel?
    private var screen: TerminalScreen?
    private var cell = CGSize(width: 8, height: 16)
    private var font = UIFont.monospacedSystemFont(ofSize: 13, weight: .regular)
    private var boldFont = UIFont.monospacedSystemFont(ofSize: 13, weight: .bold)
    private var lastGrid: (UInt16, UInt16) = (0, 0)
    private var scrollCarry: CGFloat = 0
    /// Ctrl or Alt from the extra keys, for the next key typed.
    fileprivate var ctrl = false { didSet { keys.update() } }
    fileprivate var alt = false { didSet { keys.update() } }
    private lazy var keys = ExtraKeys(terminal: self)

    private let foreground = UIColor(white: 0.92, alpha: 1)
    private let background = UIColor(red: 0.07, green: 0.08, blue: 0.06, alpha: 1)

    init(model: TerminalModel) {
        self.model = model
        super.init(frame: .zero)
        backgroundColor = background
        isOpaque = true
        contentMode = .redraw
        fontChanged()
        addGestureRecognizer(UITapGestureRecognizer(target: self, action: #selector(tapped)))
        let pan = UIPanGestureRecognizer(target: self, action: #selector(panned(_:)))
        pan.maximumNumberOfTouches = 1
        addGestureRecognizer(pan)
    }

    required init?(coder: NSCoder) { fatalError("not used") }

    func fontChanged() {
        let size = model?.fontSize ?? 13
        font = UIFont.monospacedSystemFont(ofSize: size, weight: .regular)
        boldFont = UIFont.monospacedSystemFont(ofSize: size, weight: .bold)
        let measured = ("M" as NSString).size(withAttributes: [.font: font])
        cell = CGSize(width: ceil(measured.width * 100) / 100, height: ceil(font.lineHeight))
        resizeIfNeeded()
        setNeedsDisplay()
    }

    func gridSize() -> (UInt16, UInt16) {
        let usable = bounds.inset(by: safeAreaInsets)
        let rows = max(4, Int(usable.height / cell.height))
        let cols = max(20, Int(usable.width / cell.width))
        return (UInt16(min(rows, 500)), UInt16(min(cols, 500)))
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        resizeIfNeeded()
    }

    private func resizeIfNeeded() {
        guard bounds.width > 0 else { return }
        let grid = gridSize()
        guard grid != lastGrid else { return }
        lastGrid = grid
        model?.session?.resize(rows: grid.0, cols: grid.1)
    }

    func show(_ screen: TerminalScreen) {
        self.screen = screen
        setNeedsDisplay()
    }

    // MARK: Drawing

    override func draw(_ rect: CGRect) {
        guard let screen, let context = UIGraphicsGetCurrentContext() else { return }
        let origin = CGPoint(x: safeAreaInsets.left, y: safeAreaInsets.top)
        for (row, runs) in screen.lines.enumerated() {
            let top = origin.y + CGFloat(row) * cell.height
            for run in runs {
                let x = origin.x + CGFloat(run.col) * cell.width
                let width = CGFloat(run.width) * cell.width
                let bg = color(run.bg, default: background)
                if bg != background {
                    context.setFillColor(bg.cgColor)
                    context.fill(CGRect(x: x, y: top, width: width, height: cell.height))
                }
                if run.text.trimmingCharacters(in: .whitespaces).isEmpty && !run.underline { continue }
                var fg = color(run.fg, default: foreground)
                if run.dim { fg = fg.withAlphaComponent(0.6) }
                var attributes: [NSAttributedString.Key: Any] = [.font: run.bold ? boldFont : font, .foregroundColor: fg]
                if run.underline { attributes[.underlineStyle] = NSUnderlineStyle.single.rawValue }
                if run.italic { attributes[.obliqueness] = 0.2 }
                let text = run.text as NSString
                if Int(run.width) == run.text.count {
                    // Draw cell by cell so every character sits on the grid.
                    for (i, character) in run.text.enumerated() {
                        (String(character) as NSString).draw(at: CGPoint(x: x + CGFloat(i) * cell.width, y: top), withAttributes: attributes)
                    }
                } else {
                    // A wide character: center it in its cells.
                    let drawn = text.size(withAttributes: attributes).width
                    text.draw(at: CGPoint(x: x + (width - drawn) / 2, y: top), withAttributes: attributes)
                }
            }
        }
        if screen.cursorVisible, screen.scrolledBack == 0 {
            context.setFillColor(UIColor.systemYellow.withAlphaComponent(isFirstResponder ? 0.7 : 0.3).cgColor)
            context.fill(CGRect(x: origin.x + CGFloat(screen.cursorCol) * cell.width, y: origin.y + CGFloat(screen.cursorRow) * cell.height, width: cell.width, height: cell.height))
        }
    }

    /// 0 is the default color, 1 and 2 the defaults swapped by inverse text; others are 0xAARRGGBB.
    private func color(_ value: UInt32, default fallback: UIColor) -> UIColor {
        switch value {
        case 0: return fallback
        case 1: return background
        case 2: return foreground
        default:
            return UIColor(
                red: CGFloat((value >> 16) & 0xFF) / 255, green: CGFloat((value >> 8) & 0xFF) / 255,
                blue: CGFloat(value & 0xFF) / 255, alpha: CGFloat((value >> 24) & 0xFF) / 255
            )
        }
    }

    // MARK: Gestures

    @objc private func tapped() {
        if !isFirstResponder { becomeFirstResponder() }
    }

    /// Dragging scrolls back through the terminal's history.
    @objc private func panned(_ gesture: UIPanGestureRecognizer) {
        let moved = gesture.translation(in: self).y
        gesture.setTranslation(.zero, in: self)
        scrollCarry += moved
        let lines = Int(scrollCarry / cell.height)
        guard lines != 0 else { return }
        scrollCarry -= CGFloat(lines) * cell.height
        _ = model?.session?.scrollBy(lines: Int32(lines))
        model?.screenChanged()
    }

    // MARK: Keyboard

    override var canBecomeFirstResponder: Bool { true }
    override var inputAccessoryView: UIView? { keys }
    var hasText: Bool { true }
    var autocorrectionType: UITextAutocorrectionType { get { .no } set {} }
    var autocapitalizationType: UITextAutocapitalizationType { get { .none } set {} }
    var spellCheckingType: UITextSpellCheckingType { get { .no } set {} }
    var smartQuotesType: UITextSmartQuotesType { get { .no } set {} }
    var smartDashesType: UITextSmartDashesType { get { .no } set {} }
    var smartInsertDeleteType: UITextSmartInsertDeleteType { get { .no } set {} }
    var keyboardType: UIKeyboardType { get { .asciiCapable } set {} }

    override func becomeFirstResponder() -> Bool {
        let became = super.becomeFirstResponder()
        setNeedsDisplay()
        return became
    }

    override func resignFirstResponder() -> Bool {
        let resigned = super.resignFirstResponder()
        setNeedsDisplay()
        return resigned
    }

    func insertText(_ text: String) {
        var typed = text.replacingOccurrences(of: "\n", with: "\r")
        if ctrl, let character = typed.unicodeScalars.first, typed.unicodeScalars.count == 1 {
            ctrl = false
            let value = character.value
            // Ctrl+letter (and @[\]^_) is the letter's code minus 64.
            if (0x40...0x7E).contains(value) {
                send(Data([UInt8((value & 0x1F))]))
                return
            }
        }
        if alt {
            alt = false
            typed = "\u{1b}" + typed
        }
        model?.session?.typeText(text: typed)
    }

    func deleteBackward() {
        send(Data([0x7F]))
    }

    func send(_ bytes: Data) {
        model?.session?.sendBytes(bytes: bytes)
    }

    fileprivate func arrow(_ letter: String) {
        let application = model?.session?.snapshot().applicationCursor == true
        send(Data(((application ? "\u{1b}O" : "\u{1b}[") + letter).utf8))
    }

    /// Arrows, Escape, and Tab from a hardware keyboard.
    override func pressesBegan(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        var handled = false
        for press in presses {
            guard let key = press.key else { continue }
            switch key.keyCode {
            case .keyboardUpArrow: arrow("A"); handled = true
            case .keyboardDownArrow: arrow("B"); handled = true
            case .keyboardRightArrow: arrow("C"); handled = true
            case .keyboardLeftArrow: arrow("D"); handled = true
            case .keyboardEscape: send(Data([0x1B])); handled = true
            case .keyboardTab: send(Data(key.modifierFlags.contains(.shift) ? "\u{1b}[Z".utf8 : "\t".utf8)); handled = true
            default:
                if key.modifierFlags.contains(.control), let scalar = key.charactersIgnoringModifiers.uppercased().unicodeScalars.first, (0x40...0x5F).contains(scalar.value) {
                    send(Data([UInt8(scalar.value & 0x1F)]))
                    handled = true
                }
            }
        }
        if !handled { super.pressesBegan(presses, with: event) }
    }
}

/// The web terminal's extra keys, above the keyboard: what a phone keyboard lacks.
private final class ExtraKeys: UIInputView {
    private weak var terminal: TerminalUIView?
    private var ctrlButton: UIButton?
    private var altButton: UIButton?

    init(terminal: TerminalUIView) {
        self.terminal = terminal
        super.init(frame: CGRect(x: 0, y: 0, width: 320, height: 46), inputViewStyle: .keyboard)
        allowsSelfSizing = true
        let scroll = UIScrollView()
        scroll.showsHorizontalScrollIndicator = false
        scroll.translatesAutoresizingMaskIntoConstraints = false
        addSubview(scroll)
        let row = UIStackView()
        row.axis = .horizontal
        row.spacing = 6
        row.translatesAutoresizingMaskIntoConstraints = false
        scroll.addSubview(row)
        NSLayoutConstraint.activate([
            scroll.leadingAnchor.constraint(equalTo: leadingAnchor),
            scroll.trailingAnchor.constraint(equalTo: trailingAnchor),
            scroll.topAnchor.constraint(equalTo: topAnchor),
            scroll.heightAnchor.constraint(equalToConstant: 46),
            scroll.bottomAnchor.constraint(equalTo: bottomAnchor),
            row.leadingAnchor.constraint(equalTo: scroll.contentLayoutGuide.leadingAnchor, constant: 6),
            row.trailingAnchor.constraint(equalTo: scroll.contentLayoutGuide.trailingAnchor, constant: -6),
            row.centerYAnchor.constraint(equalTo: scroll.centerYAnchor),
            row.heightAnchor.constraint(equalToConstant: 36),
        ])
        let keys: [(String, () -> Void)] = [
            ("Esc", { [weak self] in self?.terminal?.send(Data([0x1B])) }),
            ("Tab", { [weak self] in self?.terminal?.send(Data("\t".utf8)) }),
            ("Ctrl", { [weak self] in self?.terminal?.ctrl.toggle() }),
            ("Alt", { [weak self] in self?.terminal?.alt.toggle() }),
            ("←", { [weak self] in self?.terminal?.arrow("D") }),
            ("↑", { [weak self] in self?.terminal?.arrow("A") }),
            ("↓", { [weak self] in self?.terminal?.arrow("B") }),
            ("→", { [weak self] in self?.terminal?.arrow("C") }),
            ("⇤", { [weak self] in self?.terminal?.send(Data("\u{1b}[Z".utf8)) }),
            ("^C", { [weak self] in self?.terminal?.send(Data([0x03])) }),
            ("Home", { [weak self] in self?.terminal?.send(Data("\u{1b}[H".utf8)) }),
            ("End", { [weak self] in self?.terminal?.send(Data("\u{1b}[F".utf8)) }),
            ("PgUp", { [weak self] in self?.terminal?.send(Data("\u{1b}[5~".utf8)) }),
            ("PgDn", { [weak self] in self?.terminal?.send(Data("\u{1b}[6~".utf8)) }),
            ("|", { [weak self] in self?.terminal?.insertText("|") }),
            ("/", { [weak self] in self?.terminal?.insertText("/") }),
            ("~", { [weak self] in self?.terminal?.insertText("~") }),
            ("-", { [weak self] in self?.terminal?.insertText("-") }),
        ]
        for (label, action) in keys {
            var configuration = UIButton.Configuration.gray()
            configuration.title = label
            configuration.cornerStyle = .medium
            configuration.contentInsets = NSDirectionalEdgeInsets(top: 4, leading: 10, bottom: 4, trailing: 10)
            let button = UIButton(configuration: configuration, primaryAction: UIAction { _ in action() })
            if label == "Ctrl" { ctrlButton = button }
            if label == "Alt" { altButton = button }
            row.addArrangedSubview(button)
        }
    }

    required init?(coder: NSCoder) { fatalError("not used") }

    /// Shows whether Ctrl and Alt are waiting for the next key.
    func update() {
        ctrlButton?.configuration = toggled(ctrlButton?.configuration, on: terminal?.ctrl == true)
        altButton?.configuration = toggled(altButton?.configuration, on: terminal?.alt == true)
    }

    private func toggled(_ configuration: UIButton.Configuration?, on: Bool) -> UIButton.Configuration? {
        guard var configuration else { return nil }
        configuration.baseBackgroundColor = on ? .tintColor : nil
        configuration.baseForegroundColor = on ? .white : nil
        return configuration
    }
}
