import SwiftUI

/// The web app's themes (web/app.css), with the same colours. Generated from the CSS: keep in
/// step with THEMES in web/app.js. "Liquid Glass" is see-through on the web; here it is solid.
struct WebTheme: Identifiable, Hashable {
    let id: String
    let name: String
    let dark: Bool
    let bg: UInt32
    let bg2: UInt32
    let bg3: UInt32
    let line: UInt32
    let text: UInt32
    let muted: UInt32
    let accent: UInt32
    let accentInk: UInt32
    let danger: UInt32
}

let webThemes: [WebTheme] = [
    WebTheme(id: "ownnas", name: "OwnNAS", dark: true, bg: 0x12140F, bg2: 0x1B1E16, bg3: 0x262A1E, line: 0x3A4030, text: 0xF4F2E8, muted: 0xA7AD9C, accent: 0xDFF25A, accentInk: 0x1C2400, danger: 0xFF6D57),
    WebTheme(id: "tokyo-night", name: "Tokyo Night", dark: true, bg: 0x1A1B26, bg2: 0x24283B, bg3: 0x32344A, line: 0x444B6A, text: 0xC0CAF5, muted: 0x787C99, accent: 0x7AA2F7, accentInk: 0x0F111A, danger: 0xF7768E),
    WebTheme(id: "catppuccin", name: "Catppuccin", dark: true, bg: 0x1E1E2E, bg2: 0x313244, bg3: 0x45475A, line: 0x585B70, text: 0xCDD6F4, muted: 0xA6ADC8, accent: 0x89B4FA, accentInk: 0x11111B, danger: 0xF38BA8),
    WebTheme(id: "catppuccin-latte", name: "Catppuccin Latte", dark: false, bg: 0xEFF1F5, bg2: 0xE6E9EF, bg3: 0xDCE0E8, line: 0xBCC0CC, text: 0x4C4F69, muted: 0x6C6F85, accent: 0x1E66F5, accentInk: 0xFFFFFF, danger: 0xD20F39),
    WebTheme(id: "everforest", name: "Everforest", dark: true, bg: 0x2D353B, bg2: 0x374247, bg3: 0x475258, line: 0x5C6A72, text: 0xD3C6AA, muted: 0x9DA9A0, accent: 0x7FBBB3, accentInk: 0x1E2326, danger: 0xE67E80),
    WebTheme(id: "gruvbox", name: "Gruvbox", dark: true, bg: 0x282828, bg2: 0x3C3836, bg3: 0x504945, line: 0x665C54, text: 0xEBDBB2, muted: 0xA89984, accent: 0x7DAEA3, accentInk: 0x1D2021, danger: 0xEA6962),
    WebTheme(id: "kanagawa", name: "Kanagawa", dark: true, bg: 0x1F1F28, bg2: 0x2A2A37, bg3: 0x363646, line: 0x54546D, text: 0xDCD7BA, muted: 0x727169, accent: 0x7E9CD8, accentInk: 0x090618, danger: 0xE82424),
    WebTheme(id: "osaka-jade", name: "Osaka Jade", dark: true, bg: 0x111C18, bg2: 0x1A2A22, bg3: 0x23372B, line: 0x53685B, text: 0xC1C497, muted: 0x8A9A80, accent: 0x509475, accentInk: 0x07110D, danger: 0xFF5345),
    WebTheme(id: "matte-black", name: "Matte Black", dark: true, bg: 0x121212, bg2: 0x1C1C1C, bg3: 0x2A2A2A, line: 0x3D3D3D, text: 0xEAEAEA, muted: 0x8A8A8D, accent: 0xE68E0D, accentInk: 0x121212, danger: 0xD35F5F),
    WebTheme(id: "rose-pine", name: "Rose Pine Dawn", dark: false, bg: 0xFAF4ED, bg2: 0xFFFAF3, bg3: 0xF2E9E1, line: 0xDFDAD9, text: 0x575279, muted: 0x9893A5, accent: 0x56949F, accentInk: 0xFAF4ED, danger: 0xB4637A),
    WebTheme(id: "liquid-glass", name: "macOS Liquid Glass", dark: false, bg: 0xEEF1F8, bg2: 0xFFFFFF, bg3: 0xF4F6FB, line: 0xD6D9E2, text: 0x1D1D1F, muted: 0x6E6E73, accent: 0x007AFF, accentInk: 0xFFFFFF, danger: 0xFF3B30),
    WebTheme(id: "windows-11", name: "Windows 11", dark: false, bg: 0xF3F3F3, bg2: 0xFBFBFB, bg3: 0xFFFFFF, line: 0xE5E5E5, text: 0x1A1A1A, muted: 0x5D5D5D, accent: 0x005FB8, accentInk: 0xFFFFFF, danger: 0xC42B1C),
    WebTheme(id: "github-light", name: "GitHub Light", dark: false, bg: 0xFFFFFF, bg2: 0xF6F8FA, bg3: 0xEAEEF2, line: 0xD0D7DE, text: 0x1F2328, muted: 0x59636E, accent: 0x0969DA, accentInk: 0xFFFFFF, danger: 0xCF222E),
    WebTheme(id: "one-light", name: "One Light", dark: false, bg: 0xFAFAFA, bg2: 0xF0F0F1, bg3: 0xE5E5E6, line: 0xD4D4D6, text: 0x383A42, muted: 0x696C77, accent: 0x4078F2, accentInk: 0xFFFFFF, danger: 0xE45649),
    WebTheme(id: "solarized-light", name: "Solarized Light", dark: false, bg: 0xFDF6E3, bg2: 0xEEE8D5, bg3: 0xE4DDC8, line: 0xD3CBB7, text: 0x475B62, muted: 0x657B83, accent: 0x268BD2, accentInk: 0xFDF6E3, danger: 0xDC322F),
    WebTheme(id: "gruvbox-light", name: "Gruvbox Light", dark: false, bg: 0xFBF1C7, bg2: 0xF2E5BC, bg3: 0xEBDBB2, line: 0xD5C4A1, text: 0x3C3836, muted: 0x7C6F64, accent: 0x076678, accentInk: 0xFBF1C7, danger: 0x9D0006),
    WebTheme(id: "everforest-light", name: "Everforest Light", dark: false, bg: 0xFDF6E3, bg2: 0xF4F0D9, bg3: 0xEFEBD4, line: 0xE0DCC7, text: 0x5C6A72, muted: 0x829181, accent: 0x3A94C5, accentInk: 0xFFFFFF, danger: 0xF85552),
    WebTheme(id: "tokyo-night-day", name: "Tokyo Night Day", dark: false, bg: 0xE1E2E7, bg2: 0xE9E9ED, bg3: 0xD0D5E3, line: 0xC4C8DA, text: 0x3760BF, muted: 0x6172B0, accent: 0x2E7DE9, accentInk: 0xFFFFFF, danger: 0xF52A65),
    WebTheme(id: "kanagawa-lotus", name: "Kanagawa Lotus", dark: false, bg: 0xF2ECBC, bg2: 0xE7DBA0, bg3: 0xE4D794, line: 0xD5CEA3, text: 0x545464, muted: 0x8A8980, accent: 0x4D699B, accentInk: 0xF2ECBC, danger: 0xC84053),
    WebTheme(id: "nord-light", name: "Nord Light", dark: false, bg: 0xECEFF4, bg2: 0xE5E9F0, bg3: 0xD8DEE9, line: 0xC5CDD9, text: 0x2E3440, muted: 0x4C566A, accent: 0x5E81AC, accentInk: 0xFFFFFF, danger: 0xBF616A),
    WebTheme(id: "flexoki-light", name: "Flexoki Light", dark: false, bg: 0xFFFCF0, bg2: 0xF2F0E5, bg3: 0xE6E4D9, line: 0xDAD8CE, text: 0x100F0F, muted: 0x6F6E69, accent: 0x205EA6, accentInk: 0xFFFCF0, danger: 0xAF3029),
    WebTheme(id: "paper", name: "Paper", dark: false, bg: 0xF4ECD8, bg2: 0xEDE3CB, bg3: 0xE3D7BA, line: 0xD3C5A3, text: 0x3B3024, muted: 0x7A6A55, accent: 0xA0522D, accentInk: 0xFFF8EC, danger: 0xB23A2A),
    WebTheme(id: "nord", name: "Nord", dark: true, bg: 0x2E3440, bg2: 0x3B4252, bg3: 0x434C5E, line: 0x4C566A, text: 0xECEFF4, muted: 0x9AA3B5, accent: 0x88C0D0, accentInk: 0x2E3440, danger: 0xBF616A),
    WebTheme(id: "dracula", name: "Dracula", dark: true, bg: 0x282A36, bg2: 0x343746, bg3: 0x44475A, line: 0x565A6E, text: 0xF8F8F2, muted: 0xA0A4B8, accent: 0xBD93F9, accentInk: 0x1E1F29, danger: 0xFF5555),
    WebTheme(id: "solarized-dark", name: "Solarized Dark", dark: true, bg: 0x002B36, bg2: 0x073642, bg3: 0x0D4352, line: 0x1F5562, text: 0xC8D1CF, muted: 0x839496, accent: 0x268BD2, accentInk: 0xFDF6E3, danger: 0xDC322F),
]

extension Color {
    init(rgb: UInt32) {
        self.init(red: Double((rgb >> 16) & 0xFF) / 255, green: Double((rgb >> 8) & 0xFF) / 255, blue: Double(rgb & 0xFF) / 255)
    }
}

extension WebTheme {
    var background: Color { Color(rgb: bg) }
    var raised: Color { Color(rgb: bg2) }
    var raisedMore: Color { Color(rgb: bg3) }
    var textColor: Color { Color(rgb: text) }
    var mutedColor: Color { Color(rgb: muted) }
    var accentColor: Color { Color(rgb: accent) }
    var lineColor: Color { Color(rgb: line) }

    static func named(_ id: String) -> WebTheme? { webThemes.first { $0.id == id } }
}

/// "system": the phone's own look, following its light or dark mode.
let systemThemeID = "system"

private struct WebThemeKey: EnvironmentKey {
    static let defaultValue: WebTheme? = nil
}

extension EnvironmentValues {
    /// The web theme chosen in Appearance, or nil for the phone's own look.
    var webTheme: WebTheme? {
        get { self[WebThemeKey.self] }
        set { self[WebThemeKey.self] = newValue }
    }
}

/// Paints a screen in the chosen theme: its background behind lists and grids, its text colour,
/// and plain rows see-through so the background shows. Does nothing for the phone's own look.
private struct ThemedScreen: ViewModifier {
    @Environment(\.webTheme) private var theme

    func body(content: Content) -> some View {
        if let theme {
            content
                .scrollContentBackground(.hidden)
                .background(theme.background.ignoresSafeArea())
                .foregroundStyle(theme.textColor)
        } else {
            content
        }
    }
}

/// A row in the chosen theme: see-through so the screen's background shows.
private struct ThemedRow: ViewModifier {
    @Environment(\.webTheme) private var theme

    func body(content: Content) -> some View {
        if let theme {
            content.listRowBackground(theme.background)
        } else {
            content
        }
    }
}

/// A row of a grouped form (settings sheets) in the chosen theme: on its raised shade.
private struct ThemedFormRow: ViewModifier {
    @Environment(\.webTheme) private var theme

    func body(content: Content) -> some View {
        if let theme {
            content.listRowBackground(theme.raised)
        } else {
            content
        }
    }
}

extension View {
    func themedScreen() -> some View { modifier(ThemedScreen()) }
    func themedRow() -> some View { modifier(ThemedRow()) }
    func themedFormRow() -> some View { modifier(ThemedFormRow()) }
}

/// The web app's themes, to colour this app the same way; or the phone's own look.
struct AppearanceView: View {
    @Environment(\.dismiss) private var dismiss
    @AppStorage("theme") private var themeID = systemThemeID

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 14) {
                    Text("The same themes as the web app. The phone's look follows its light or dark mode.")
                        .font(.footnote).foregroundStyle(.secondary)
                    Button { themeID = systemThemeID } label: {
                        HStack {
                            Image(systemName: "iphone").font(.title3)
                            VStack(alignment: .leading) {
                                Text("Phone").foregroundStyle(.primary)
                                Text("Follows light or dark mode").font(.caption).foregroundStyle(.secondary)
                            }
                            Spacer()
                            if themeID == systemThemeID { Image(systemName: "checkmark.circle.fill") }
                        }
                        .padding(14)
                        .background(RoundedRectangle(cornerRadius: 14).fill(Color(uiColor: .secondarySystemBackground)))
                        .overlay(RoundedRectangle(cornerRadius: 14).stroke(themeID == systemThemeID ? Color.accentColor : .clear, lineWidth: 2))
                    }
                    .buttonStyle(.plain)
                    section("Dark", webThemes.filter(\.dark))
                    section("Light", webThemes.filter { !$0.dark })
                }
                .padding()
            }
            .navigationTitle("Appearance")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } } }
        }
    }

    private func section(_ title: String, _ themes: [WebTheme]) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(title).font(.headline)
            LazyVGrid(columns: [GridItem(.adaptive(minimum: 150), spacing: 10)], spacing: 10) {
                ForEach(themes) { theme in
                    Button { themeID = theme.id } label: { card(theme) }.buttonStyle(.plain)
                }
            }
        }
    }

    /// A theme drawn in its own colours: a little window with a bar, text lines, and its accent.
    private func card(_ theme: WebTheme) -> some View {
        let chosen = themeID == theme.id
        return VStack(alignment: .leading, spacing: 0) {
            theme.raised.frame(height: 20)
            VStack(alignment: .leading, spacing: 5) {
                Capsule().fill(theme.textColor).frame(width: 90, height: 6)
                Capsule().fill(theme.mutedColor).frame(width: 60, height: 6)
                HStack {
                    Capsule().fill(theme.accentColor).frame(width: 44, height: 14)
                    Spacer()
                    if chosen { Image(systemName: "checkmark.circle.fill").foregroundStyle(theme.accentColor) }
                }
                Text(theme.name).font(.footnote).foregroundStyle(theme.textColor).lineLimit(1)
            }
            .padding(10)
        }
        .background(theme.background)
        .clipShape(RoundedRectangle(cornerRadius: 14))
        .overlay(RoundedRectangle(cornerRadius: 14).stroke(chosen ? theme.accentColor : theme.lineColor, lineWidth: chosen ? 3 : 1))
    }
}
