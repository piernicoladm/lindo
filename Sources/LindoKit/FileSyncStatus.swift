import Foundation

/// Captured during a scan so review never has to contact the filesystem or a provider.
public enum FileSyncStatus: Equatable, Sendable {
    case local
    case synced
    case unknown

    /// An incomplete classification must not suppress the warning for a confirmed synced file.
    public static func combined<S: Sequence>(_ statuses: S) -> FileSyncStatus where S.Element == FileSyncStatus {
        var result = FileSyncStatus.local
        for status in statuses {
            if status == .synced { return .synced }
            if status == .unknown { result = .unknown }
        }
        return result
    }
}

internal enum FileSyncPolicy {
    static func isKnownSyncedPath(_ path: String, home: String) -> Bool {
        let folded = path.lowercased()
        return ["/Library/CloudStorage", "/Library/Mobile Documents", "/Dropbox", "/OneDrive", "/Google Drive", "/Box", "/pCloud Drive"]
            .contains {
                let root = (home + $0).lowercased()
                return folded == root || folded.hasPrefix(root + "/")
            }
    }

    /// A missing metadata answer is unknown, even if no known provider path matches.
    static func classify(path: String, home: String, isUbiquitous: Bool?) -> FileSyncStatus {
        if isKnownSyncedPath(path, home: home) || isUbiquitous == true { return .synced }
        return isUbiquitous == false ? .local : .unknown
    }

    /// Call only from a filesystem worker. The injectable reader keeps failure behavior testable.
    static func read(from url: URL, home: String,
                     metadata: (URL) throws -> Bool? = { url in
                         try url.resourceValues(forKeys: [.isUbiquitousItemKey]).isUbiquitousItem
                     }) -> FileSyncStatus {
        if isKnownSyncedPath(url.path, home: home) { return .synced }
        do { return classify(path: url.path, home: home, isUbiquitous: try metadata(url)) }
        catch { return .unknown }
    }
}
