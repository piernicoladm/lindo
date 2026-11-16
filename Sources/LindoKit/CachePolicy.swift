import Foundation

/// Only complete HTTP cache units are eligible; other browser data is never traversed.
enum CachePolicy {
    struct Owner: Equatable, Sendable {
        let identifier: String
        let title: String
        let applicationName: String
    }
    private static let chrome = Owner(identifier: "com.google.Chrome", title: "Google Chrome", applicationName: "Google Chrome.app")
    private static let chromium = Owner(identifier: "org.chromium.Chromium", title: "Chromium", applicationName: "Chromium.app")
    private static let firefox = Owner(identifier: "org.mozilla.firefox", title: "Firefox", applicationName: "Firefox.app")
    private static func components(_ url: URL, root: URL) -> [String]? {
        let path = url.standardizedFileURL.path, base = root.standardizedFileURL.path
        guard path == base || path.hasPrefix(base + "/") else { return nil }
        return Array(url.standardizedFileURL.pathComponents.dropFirst(root.standardizedFileURL.pathComponents.count))
    }
    private static func profile(_ name: String) -> Bool {
        name == "Default" || name.range(of: #"^Profile [0-9]+$"#, options: .regularExpression) != nil
    }
    static func owner(of url: URL, root: URL) -> Owner? {
        guard let c = components(url, root: root) else { return nil }
        if c.count == 4, Array(c.prefix(2)) == ["Google", "Chrome"], profile(c[2]), c[3] == "Cache" { return chrome }
        if c.count == 3, c[0] == "Chromium", profile(c[1]), c[2] == "Cache" { return chromium }
        if c.count == 4, Array(c.prefix(2)) == ["Firefox", "Profiles"], !c[2].isEmpty, !c[2].hasPrefix("."), c[3] == "cache2" { return firefox }
        return nil
    }
    static func mayTraverse(_ url: URL, root: URL) -> Bool {
        guard let c = components(url, root: root) else { return false }
        if c.isEmpty { return true }
        switch c[0] {
        case "Google": return c.count == 1 || (c.count == 2 && c[1] == "Chrome") || (c.count == 3 && c[1] == "Chrome" && profile(c[2]))
        case "Chromium": return c.count == 1 || (c.count == 2 && profile(c[1]))
        case "Firefox": return c.count == 1 || (c.count == 2 && c[1] == "Profiles") || (c.count == 3 && c[1] == "Profiles" && !c[2].hasPrefix("."))
        default: return false
        }
    }
    static func isRunning(_ owner: Owner, paths: Set<String>, identifiers: Set<String>) -> Bool {
        if owner == firefox {
            // Channels share the Firefox cache root, and helpers may outlive the UI.
            let channels = ["org.mozilla.firefox", "org.mozilla.firefoxdeveloperedition", "org.mozilla.nightly"]
            let helperBrands = ["firefox", "firefox-developer-edition", "firefox-nightly", "nightly"]
            let helpers = ["gpu-helper", "media-plugin-helper", "security-module-helper"]
            let familyIdentifiers = channels + ["org.mozilla.plugincontainer"] + helperBrands.flatMap { brand in
                helpers.map { "org.mozilla.\(brand)-\($0)" }
            }
            if identifiers.contains(where: { identifier in
                familyIdentifiers.contains { identifier == $0 || identifier.hasPrefix($0 + ".") }
            }) { return true }
            let applications: Set<String> = ["Firefox.app", "Firefox Developer Edition.app", "Firefox Nightly.app", "Nightly.app"]
            let executables: Set<String> = ["firefox", "firefox-bin", "plugin-container", "gpu-helper", "media-plugin-helper", "security-module-helper"]
            return paths.contains { path in
                guard path.hasPrefix("/") else { return false }
                let c = URL(fileURLWithPath: path).standardizedFileURL.pathComponents
                guard c.count >= 5, c[c.count - 4].hasSuffix(".app"), c[c.count - 3] == "Contents", c[c.count - 2] == "MacOS" else { return false }
                // Executable names survive app renaming. Shared Gecko helper names
                // intentionally err toward blocking when ownership is ambiguous.
                return executables.contains(c[c.count - 1]) || c.contains(where: applications.contains)
            }
        }
        if identifiers.contains(where: { $0 == owner.identifier || $0.hasPrefix(owner.identifier + ".") }) { return true }
        return paths.contains { path in
            let c = URL(fileURLWithPath: path).standardizedFileURL.pathComponents
            if c.contains(owner.applicationName) { return true }
            guard path.hasPrefix("/"), c.count >= 5, c[c.count - 4].hasSuffix(".app"),
                  c[c.count - 3] == "Contents", c[c.count - 2] == "MacOS" else { return false }
            // Executable names remain stable when the enclosing app is renamed.
            let executable = c[c.count - 1]
            let browser = owner == chrome ? "Google Chrome" : "Chromium"
            let names = [browser, browser + " Helper", browser + " Helper (Renderer)",
                         browser + " Helper (GPU)", browser + " Helper (Plugin)", browser + " Helper (Alerts)"]
            return names.contains(executable)
        }
    }
}
