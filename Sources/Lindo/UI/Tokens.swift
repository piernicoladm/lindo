import SwiftUI

// Semantic system colors keep content neutral and track macOS appearance settings.
enum Palette {
    static let accent = Color(nsColor: .controlAccentColor)
    static let accentText = Color(nsColor: .linkColor)
    static let canvas = Color(nsColor: .windowBackgroundColor)
    static let surface = Color(nsColor: .textBackgroundColor)
    static let secondarySurface = Color(nsColor: .controlBackgroundColor)
    static let line = Color(nsColor: .separatorColor)
}
