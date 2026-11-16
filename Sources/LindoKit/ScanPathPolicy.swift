import Foundation

/// Shared lexical exclusions. Scope and no-follow filesystem checks remain
/// separate, and are still revalidated immediately before moving anything.
internal enum ScanPathPolicy {
    // Directory documents stay closed even when no installed app registers their type.
    static let packages: Set<String> = [
        "app", "bundle", "framework", "plugin", "photoslibrary", "photolibrary", "pages", "numbers", "key", "rtfd", "band", "logicx", "sparsebundle", "playground", "xcassets",
        "vmwarevm", "pvm", "utm", "backupdb", "aplibrary", "migratedphotolibrary", "imovielibrary", "tvlibrary", "musiclibrary", "theater", "fcpbundle", "dvdmedia",
        "xcarchive", "xcodeproj", "xcworkspace", "xcdatamodeld", "dsym", "docset", "playgroundbook", "kext", "prefpane", "saver", "qlgenerator", "mdimporter", "appex", "xpc",
        "component", "vst", "vst3", "workflow", "action", "wdgt", "pkg", "mpkg", "download", "scriv", "textbundle", "graffle", "oo3", "ofocus", "dtbase2", "lrdata", "lrcat-data"
    ]
    private static let forbidden: Set<String> = ["_6_archivio_private", "piernicola de maria", ".codex", ".ssh", "keychains", "mail", "messages", "photos", ".trash", ".trashes", "system"]
    private static let appResources: Set<String> = ["mail", "messages", "photos", "system"]

    private static func within(_ path: String, _ root: String) -> Bool {
        path == root || path.hasPrefix(root + "/")
    }

    static func isProtected(_ url: URL, applicationRoot: URL? = nil) -> Bool {
        let parts = url.pathComponents.map { $0.lowercased() }
        let resourceStart = applicationRoot.flatMap { root -> Int? in
            guard within(url.path, root.path), !isProtected(root) else { return nil }
            return root.pathComponents.count
        }
        if parts.enumerated().contains(where: { index, part in
            forbidden.contains(part) && !(resourceStart.map { index >= $0 && appResources.contains(part) } ?? false)
        }) { return true }
        if zip(parts, parts.dropFirst()).contains(where: { $0 == "_2_work" && $1 == "clients" }) { return true }
        if resourceStart == nil && parts.contains(where: { $0.hasPrefix("com.apple.") || $0 == "apple" }) { return true }
        // Fixed system locations fold letter case, like the name rules above.
        let folded = url.path.lowercased()
        return within(folded, "/library/apple") || within(folded, "/applications/utilities")
    }
}
