import Foundation
import Darwin

public enum ScanCategory: String, CaseIterable, Codable, Sendable {
    case caches, logs, largeFiles, applications
}

public struct ScanItem: Identifiable, Sendable {
    public let id: String
    public let url: URL
    public let title: String
    public let bytes: Int64
    public let modifiedAt: Date
    public let category: ScanCategory
    public let detail: String
    public let ownerBundleIdentifier: String?
    public let syncStatus: FileSyncStatus

    public init(id: String = UUID().uuidString, url: URL, title: String, bytes: Int64, modifiedAt: Date = Date(), category: ScanCategory, detail: String = "", ownerBundleIdentifier: String? = nil, syncStatus: FileSyncStatus = .unknown) {
        self.id = id; self.url = url; self.title = title; self.bytes = bytes
        self.modifiedAt = modifiedAt; self.category = category; self.detail = detail
        self.ownerBundleIdentifier = ownerBundleIdentifier
        self.syncStatus = syncStatus
    }
}

public struct ScanReport: Sendable {
    public let items: [ScanItem]
    public let warnings: [String]
    public let inspectedCount: Int
    public let isPartial: Bool
    public let cancelled: Bool
    public let duration: TimeInterval
}

public struct RunningApplications: Sendable {
    public let paths: Set<String>
    public let bundleIdentifiers: Set<String>
    public let inspectionComplete: Bool

    public init(paths: Set<String>, bundleIdentifiers: Set<String>, inspectionComplete: Bool = true) {
        self.paths = paths
        self.bundleIdentifiers = bundleIdentifiers
        self.inspectionComplete = inspectionComplete
    }
}

public struct TrashReport: Sendable {
    public let trashedIDs: [String]
    public let failures: [String]
    public let bytes: Int64
}

private struct Identity: Equatable, Sendable {
    let device: Int32
    let inode: UInt64
    let modifiedSeconds: Int
    let modifiedNanoseconds: Int
    let changedSeconds: Int
    let changedNanoseconds: Int
    let size: Int64
    let mode: UInt16
    let owner: uid_t
    let linkCount: UInt16

    init(_ info: stat) {
        owner = info.st_uid
        linkCount = info.st_nlink
        device = info.st_dev; inode = info.st_ino; size = info.st_size; mode = info.st_mode
        modifiedSeconds = info.st_mtimespec.tv_sec
        modifiedNanoseconds = info.st_mtimespec.tv_nsec
        changedSeconds = info.st_ctimespec.tv_sec
        changedNanoseconds = info.st_ctimespec.tv_nsec
    }
    func sameNode(as other: Identity?) -> Bool {
        guard let other else { return false }
        return device == other.device && inode == other.inode && mode == other.mode
    }
    var isDirectory: Bool { mode & S_IFMT == S_IFDIR }
    var isRegular: Bool { mode & S_IFMT == S_IFREG }
}

private enum TraversalError: Error { case limit }

private struct NodeKey: Hashable {
    let device: Int32
    let inode: UInt64
}

private struct Candidate: Sendable {
    let item: ScanItem
    let identity: Identity
    let root: URL
    let rootIdentity: Identity
    let appBundleIdentifier: String
    let tree: [String: Identity]
}

public actor MacScanner {
    private var candidates: [String: Candidate] = [:]
    private var trashingIDs: Set<String> = []
    private var generations: [ScanCategory: UUID] = [:]
    private var scanTasks: [ScanCategory: Task<(ScanReport, [String: Candidate]), any Error>] = [:]
    private let entryLimit: Int
    private let fixtureScopes: Bool
    private let cacheRoot: URL
    private let processInventory: @Sendable () -> ProcessSnapshot
    private let trash: @Sendable (URL) throws -> Void
    private nonisolated var fm: FileManager { .default }
    private let readExecutor: ScanReadExecutor
    private let beforeScanRead: (@Sendable () -> Void)?
    private let afterDirectoryEnumeration: (@Sendable (URL) -> Void)?
    private let ownBundlePath: String

    public init() {
        ownBundlePath = Bundle.main.bundleURL.standardizedFileURL.path
        readExecutor = .shared
        beforeScanRead = nil
        afterDirectoryEnumeration = nil
        processInventory = ProcessInventory.capture
        entryLimit = Int.max; fixtureScopes = false
        cacheRoot = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Caches")
        trash = { url in try FileManager.default.trashItem(at: url, resultingItemURL: nil) }
    }

    // Only accessible to the test target. Production cannot opt out of scope rules.
    init(entryLimit: Int = Int.max, fixtureScopes: Bool = true, cacheRoot: URL? = nil, ownBundlePath: String? = nil, beforeScanRead: (@Sendable () -> Void)? = nil, afterDirectoryEnumeration: (@Sendable (URL) -> Void)? = nil, readExecutor: ScanReadExecutor = ScanReadExecutor(), processInventory: @escaping @Sendable () -> ProcessSnapshot = { ProcessSnapshot(paths: [], isComplete: true) }, trash: @escaping @Sendable (URL) throws -> Void) {
        self.readExecutor = readExecutor
        self.beforeScanRead = beforeScanRead
        self.afterDirectoryEnumeration = afterDirectoryEnumeration
        self.ownBundlePath = URL(fileURLWithPath: ownBundlePath ?? Bundle.main.bundleURL.path).standardizedFileURL.path
        self.processInventory = processInventory
        self.cacheRoot = cacheRoot ?? FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Caches")
        self.entryLimit = max(1, entryLimit); self.fixtureScopes = fixtureScopes; self.trash = trash
    }

    private nonisolated func identity(_ url: URL) throws -> Identity {
        var info = stat()
        guard lstat(url.path, &info) == 0 else { throw CocoaError(.fileReadNoPermission) }
        return Identity(info)
    }

    private nonisolated func inside(_ path: String, _ root: String) -> Bool {
        path == root || path.hasPrefix(root + "/")
    }

    private nonisolated func protected(_ url: URL, applicationRoot: URL? = nil) -> Bool {
        ScanPathPolicy.isProtected(url, applicationRoot: applicationRoot)
    }

    private nonisolated func safeAncestors(_ url: URL) -> Bool {
        var cursor = url.standardizedFileURL
        while true {
            guard let info = try? identity(cursor), info.mode & S_IFMT != S_IFLNK else { return false }
            if cursor != url && package(cursor, isDirectory: info.isDirectory) { return false }
            if cursor.path == "/" { return true }
            cursor.deleteLastPathComponent()
        }
    }

    private nonisolated func directoryContents(_ url: URL) throws -> [URL] {
        let children = try fm.contentsOfDirectory(at: url, includingPropertiesForKeys: nil)
        // The test-only hook can replace an ancestor after its children are queued.
        afterDirectoryEnumeration?(url)
        return children
    }

    /// Uses the on-disk spelling when a root differs from it only by letter case.
    /// Enumeration returns children under the real spelling: without this they fall
    /// outside the root they were listed from and the scan is silently empty.
    private nonisolated func onDiskSpelling(_ url: URL) -> URL {
        let standardized = url.standardizedFileURL
        guard let canonical = try? standardized.resourceValues(forKeys: [.canonicalPathKey]).canonicalPath,
              canonical != standardized.path, canonical.lowercased() == standardized.path.lowercased() else { return standardized }
        return URL(fileURLWithPath: canonical, isDirectory: true)
    }

    private nonisolated func placeholder(_ url: URL) -> Bool {
        if url.lastPathComponent.hasSuffix(".icloud") { return true }
        guard let values = try? url.resourceValues(forKeys: [.isUbiquitousItemKey, .ubiquitousItemDownloadingStatusKey]) else { return true }
        if values.isUbiquitousItem == true && values.ubiquitousItemDownloadingStatus != .current { return true }
        var info = stat()
        if lstat(url.path, &info) != 0 { return true }
        // SF_DATALESS: File Provider content is not materialized.
        return info.st_flags & UInt32(0x40000000) != 0
    }

    private nonisolated func package(_ url: URL, isDirectory known: Bool? = nil) -> Bool {
        // A package is a directory shown as one document. A flat file such as an
        // installer .pkg is an ordinary file; an unreadable kind stays closed.
        guard known ?? ((try? identity(url).isDirectory) ?? true) else { return false }
        if ScanPathPolicy.packages.contains(url.pathExtension.lowercased()) { return true }
        // An unreadable answer is treated as a package: never descend on doubt.
        guard let values = try? url.resourceValues(forKeys: [.isPackageKey]) else { return true }
        return values.isPackage == true
    }

    // Pure scope policy, separate from filesystem checks so cloud paths can be tested without touching providers.
    static func allowsLargeFilePath(_ path: String, home: String) -> Bool {
        func within(_ root: String) -> Bool { path == root || path.hasPrefix(root + "/") }
        // Deny rules fold letter case: on the default case-insensitive volume
        // "library" names the real Library. Allow rules stay exact.
        let folded = path.lowercased()
        func foldedWithin(_ root: String) -> Bool {
            let foldedRoot = root.lowercased()
            return folded == foldedRoot || folded.hasPrefix(foldedRoot + "/")
        }
        guard folded != home.lowercased(),
              !["/usr", "/bin", "/sbin", "/private", "/Library", "/System", "/opt"].contains(where: foldedWithin),
              !URL(fileURLWithPath: path).pathComponents.dropFirst().contains(where: { $0.hasPrefix(".") }) else { return false }
        if foldedWithin(home + "/Library") {
            return foldedWithin(home + "/Library/CloudStorage") || foldedWithin(home + "/Library/Mobile Documents")
        }
        return within(home) || within("/Volumes")
    }

    private nonisolated func allowedRoot(_ root: URL, category: ScanCategory) -> Bool {
        guard !protected(root), safeAncestors(root), !placeholder(root), !package(root),
              (try? identity(root).isDirectory) == true else { return false }
        let home = fm.homeDirectoryForCurrentUser.path
        let foldedRoot = root.path.lowercased()
        if ["/", home, "/Library", home + "/Library", "/Applications"].contains(where: { $0.lowercased() == foldedRoot }) && category != .applications { return false }
        if category == .caches { return CachePolicy.mayTraverse(root, root: cacheRoot) || CachePolicy.owner(of: root, root: cacheRoot) != nil }
        if fixtureScopes { return true }
        switch category {
        case .largeFiles: return Self.allowsLargeFilePath(root.path, home: home)
        case .applications: return ["/Applications", home + "/Applications"].contains(root.path)
        case .caches: return ["/Library/Caches", home + "/Library/Caches"].contains(where: { inside(root.path, $0) })
        case .logs: return ["/Library/Logs", home + "/Library/Logs"].contains(where: { inside(root.path, $0) })
        }
    }

    public func scan(category: ScanCategory, roots: [URL], minimumBytes: Int64 = 100_000_000, olderThan: Date? = nil) async -> ScanReport {
        let started = Date()
        let token = UUID(); generations[category] = token
        let rescannedRoots = roots.map { onDiskSpelling($0).path }
        candidates = candidates.filter { _, value in
            value.item.category != category || !rescannedRoots.contains(where: {
                inside(value.root.path, $0) || inside($0, value.root.path)
            })
        }
        scanTasks[category]?.cancel()
        let task = Task { [self] in
            try await readExecutor.run { [self] cancellation in
                try collectScan(category: category, roots: roots, minimumBytes: minimumBytes,
                                olderThan: olderThan, started: started, cancellation: cancellation)
            }
        }
        scanTasks[category] = task
        do {
            let (report, collected) = try await withTaskCancellationHandler {
                try await task.value
            } onCancel: { task.cancel() }
            guard !Task.isCancelled, generations[category] == token else { throw CancellationError() }
            candidates.merge(collected) { _, latest in latest }
            scanTasks[category] = nil
            return report
        } catch {
            if generations[category] == token { scanTasks[category] = nil }
            let cancelled = error is CancellationError || Task.isCancelled || generations[category] != token
            let message = cancelled ? "Scansione interrotta." : "Letture precedenti ancora in attesa del sistema. Riprova o scegli una cartella locale."
            return ScanReport(items: [], warnings: [message], inspectedCount: 0, isPartial: true,
                              cancelled: cancelled, duration: Date().timeIntervalSince(started))
        }
    }

    private nonisolated func collectScan(category: ScanCategory, roots: [URL], minimumBytes: Int64,
                                        olderThan: Date?, started: Date, cancellation: ScanReadCancellation) throws -> (ScanReport, [String: Candidate]) {
        try cancellation.check()
        beforeScanRead?()
        try cancellation.check()
        var localCandidates: [String: Candidate] = [:]
        var items: [ScanItem] = [], warnings: [String] = []
        var count = 0, applicationDiscoveryCount = 0, limited = false
        var hiddenExclusions = 0, packageExclusions = 0
        // Only directories and multiply linked files can be reached twice.
        var visited: Set<NodeKey> = []
        let normalized = Array(Set(roots.map { onDiskSpelling($0) })).sorted { $0.path.count < $1.path.count }
        var accepted: [URL] = []
        for root in normalized {
            if !allowedRoot(root, category: category) {
                warnings.append("Cartella esclusa o non accessibile: \(root.path)"); continue
            }
            if !accepted.contains(where: { inside(root.path, $0.path) }) { accepted.append(root) }
        }
        for root in accepted {
            guard let rootIdentity = try? identity(root) else { warnings.append("Cartella non più accessibile: \(root.path)"); continue }
            // The same folder named twice, for example in another letter case, is scanned once.
            // A cache root is itself the first visited entry, so it is recorded by the walk below.
            guard category == .caches || visited.insert(NodeKey(device: rootIdentity.device, inode: rootIdentity.inode)).inserted else { continue }
            var stack: [URL] = []
            do { stack = category == .caches ? [root] : try directoryContents(root) }
            catch { warnings.append("Impossibile leggere: \(root.path)"); continue }
            while let url = stack.popLast() {
                try cancellation.check()
                if (category == .applications ? applicationDiscoveryCount : count) >= entryLimit { limited = true; break }
                count += 1
                if category == .applications { applicationDiscoveryCount += 1 }
                try cancellation.check()
                guard inside(url.path, root.path) else { continue }
                if protected(url) {
                    warnings.append("Elemento escluso dalle protezioni: \(url.path)")
                    continue
                }
                if (category == .largeFiles || category == .logs), url.pathComponents.contains(where: { $0.hasPrefix(".") }) {
                    hiddenExclusions += 1
                    continue
                }
                // A previously enumerated directory can now be a symlink. Recheck
                // every ancestor before inspecting any queued child.
                guard safeAncestors(url), !placeholder(url) else {
                    warnings.append("Elemento escluso o non accessibile: \(url.path). Collegamento, contenuto non locale o percorso cambiato.")
                    continue
                }
                let info: Identity
                do { info = try identity(url) }
                catch { warnings.append("Elemento non accessibile: \(url.path)"); continue }
                if info.isDirectory || info.linkCount > 1 {
                    guard visited.insert(NodeKey(device: info.device, inode: info.inode)).inserted else { continue }
                }
                if category == .caches {
                    guard info.isDirectory, info.owner == getuid(), !package(url, isDirectory: true) else { continue }
                    if let owner = CachePolicy.owner(of: url, root: cacheRoot) {
                        var budget = entryLimit - count
                        do {
                            let tree = try cacheTree(url, budget: &budget, check: cancellation.check)
                            count = entryLimit - budget
                            let size = uniqueFileBytes(in: tree)
                            let item = ScanItem(url: url, title: "Cache HTTP · \(owner.title)", bytes: size, category: .caches, detail: "Intera cache HTTP. Chiudi il browser prima di pulire", ownerBundleIdentifier: owner.identifier, syncStatus: FileSyncPolicy.read(from: url, home: fm.homeDirectoryForCurrentUser.path))
                            items.append(item)
                            localCandidates[item.id] = Candidate(item: item, identity: info, root: root, rootIdentity: rootIdentity, appBundleIdentifier: owner.identifier, tree: tree)
                        } catch {
                            count = entryLimit - budget
                            warnings.append("Cache esclusa: \(url.path). Contenuti non verificabili.")
                            if error is TraversalError { limited = true; break }
                        }
                    } else if CachePolicy.mayTraverse(url, root: cacheRoot) {
                        do { stack.append(contentsOf: try directoryContents(url)) }
                        catch { warnings.append("Impossibile leggere: \(url.path)") }
                    }
                    continue
                }
                if category == .applications {
                    guard info.isDirectory else { continue }
                    guard url.pathExtension.lowercased() == "app" else {
                        guard !package(url, isDirectory: true) else { continue }
                        let depth = url.pathComponents.count - root.pathComponents.count
                        guard depth <= 3 else {
                            warnings.append("Sottocartella esclusa: \(url.path). Profondità massima raggiunta.")
                            continue
                        }
                        do { stack.append(contentsOf: try directoryContents(url)) }
                        catch { warnings.append("Impossibile leggere: \(url.path)") }
                        continue
                    }
                    // Excluded bundles do not consume a whole-tree verification budget.
                    let preview = applicationMetadata(url)
                    count += 1
                    // An unreadable identity cannot prove the bundle is third-party: exclude it.
                    guard !preview.identifier.isEmpty else {
                        warnings.append("Applicazione esclusa: \(url.lastPathComponent). Identità del bundle non leggibile.")
                        continue
                    }
                    guard !preview.identifier.hasPrefix("com.apple."), preview.identifier != "com.piernicolademaria.lindo" else { continue }
                    var budget = entryLimit
                    do {
                        let tree = try applicationTree(url, budget: &budget, check: cancellation.check)
                        try cancellation.check()
                        count += entryLimit - budget
                        let size = uniqueFileBytes(in: tree)
                        let metadata = applicationMetadata(url, tree: tree)
                        let version = metadata.version
                        guard metadata.identifier == preview.identifier else {
                            warnings.append("Applicazione esclusa: \(url.lastPathComponent). Identità del bundle cambiata durante l'analisi.")
                            continue
                        }
                        let item = ScanItem(id: UUID().uuidString, url: url, title: url.deletingPathExtension().lastPathComponent, bytes: size, modifiedAt: Date(timeIntervalSince1970: Double(info.modifiedSeconds)), category: category, detail: version, syncStatus: FileSyncPolicy.read(from: url, home: fm.homeDirectoryForCurrentUser.path))
                        items.append(item); localCandidates[item.id] = Candidate(item: item, identity: info, root: root, rootIdentity: rootIdentity, appBundleIdentifier: metadata.identifier, tree: tree)
                    } catch {
                        warnings.append("Applicazione esclusa: \(url.lastPathComponent). Contenuti non verificabili o limite raggiunto.")
                        count += entryLimit - budget
                        try cancellation.check()
                        // A single incomplete bundle must not hide other applications.
                    }
                    continue
                }
                if package(url, isDirectory: info.isDirectory) {
                    packageExclusions += 1
                    continue
                }
                if info.isDirectory {
                    do { stack.append(contentsOf: try directoryContents(url)) }
                    catch { warnings.append("Impossibile leggere: \(url.path)") }
                    continue
                }
                guard info.isRegular, info.size >= (category == .largeFiles ? max(0, minimumBytes) : 0) else { continue }
                let date = Date(timeIntervalSince1970: Double(info.modifiedSeconds))
                if let olderThan, date >= olderThan { continue }
                let label = category == .caches ? "Cache applicazione" : category == .logs ? "Log applicazione" : "File locale"
                let item = ScanItem(id: UUID().uuidString, url: url, title: url.lastPathComponent, bytes: info.size, modifiedAt: date, category: category, detail: label, syncStatus: FileSyncPolicy.read(from: url, home: fm.homeDirectoryForCurrentUser.path))
                items.append(item); localCandidates[item.id] = Candidate(item: item, identity: info, root: root, rootIdentity: rootIdentity, appBundleIdentifier: "", tree: [:])
            }
            if limited { break }
        }
        if hiddenExclusions > 0 { warnings.append("Elementi nascosti esclusi: \(hiddenExclusions). Il contenuto delle cartelle nascoste non è stato analizzato.") }
        if packageExclusions > 0 { warnings.append("Pacchetti esclusi: \(packageExclusions). Il loro contenuto non è stato analizzato.") }
        if limited { warnings.append("Limite di scansione raggiunto. Restringi la cartella selezionata.") }
        try cancellation.check()
        return (ScanReport(items: items.sorted { $0.bytes > $1.bytes }, warnings: warnings, inspectedCount: count, isPartial: !warnings.isEmpty, cancelled: false, duration: Date().timeIntervalSince(started)), localCandidates)
    }

    private nonisolated func cacheTree(_ root: URL, budget: inout Int, check: () throws -> Void = { try Task.checkCancellation() }) throws -> [String: Identity] {
        var stack = [root], tree: [String: Identity] = [:]
        while let url = stack.popLast() {
            guard budget > 0 else { throw TraversalError.limit }
            budget -= 1
            try check()
            guard !protected(url), safeAncestors(url), !placeholder(url), !package(url) else { throw CocoaError(.fileReadNoPermission) }
            let info = try identity(url)
            guard info.owner == getuid(), info.isDirectory || info.isRegular else { throw CocoaError(.fileReadNoPermission) }
            tree[url.path] = info
            if info.isDirectory { stack.append(contentsOf: try fm.contentsOfDirectory(at: url, includingPropertiesForKeys: nil)) }
        }
        return tree
    }

    private nonisolated func uniqueFileBytes(in tree: [String: Identity]) -> Int64 {
        var visited: Set<String> = []
        return tree.values.reduce(0) { total, entry in
            guard entry.isRegular, visited.insert("\(entry.device):\(entry.inode)").inserted else { return total }
            return total + entry.size
        }
    }

    // Packages inside an app are legitimate, but no queued child may be read through
    // a directory that became a symbolic link while the actor was suspended.
    static func applicationAncestorsAreDirectories(_ url: URL, root: URL) -> Bool {
        guard url.path == root.path || url.path.hasPrefix(root.path + "/") else { return false }
        var cursor = url == root ? root : url.deletingLastPathComponent()
        while true {
            var info = stat()
            guard lstat(cursor.path, &info) == 0, info.st_mode & S_IFMT == S_IFDIR else { return false }
            if cursor == root { return true }
            guard cursor.path != "/" else { return false }
            cursor.deleteLastPathComponent()
        }
    }

    private nonisolated func applicationTree(_ root: URL, budget: inout Int, check: () throws -> Void = { try Task.checkCancellation() }) throws -> [String: Identity] {
        var stack = [root], tree: [String: Identity] = [:]
        while let url = stack.popLast() {
            guard budget > 0 else { throw TraversalError.limit }
            budget -= 1
            try check()
            guard safeAncestors(root), Self.applicationAncestorsAreDirectories(url, root: root),
                  !protected(url, applicationRoot: root), !placeholder(url) else { throw CocoaError(.fileReadNoPermission) }
            let info = try identity(url)
            // Symbolic links inside signed apps are common. Record their identity without following them.
            tree[url.path] = info
            if info.isDirectory { stack.append(contentsOf: try fm.contentsOfDirectory(at: url, includingPropertiesForKeys: nil)) }
        }
        return tree
    }

    private nonisolated func applicationMetadata(_ root: URL, tree: [String: Identity]? = nil) -> (version: String, identifier: String) {
        let unknown = (version: "Versione non disponibile", identifier: "")
        guard safeAncestors(root), !protected(root) else { return unknown }
        // Open every directory component without following symlinks. Directory
        // descriptors anchor the read even if a parent is renamed concurrently.
        var directory = open("/", O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC)
        guard directory >= 0 else { return unknown }
        defer { close(directory) }
        for component in root.pathComponents.dropFirst() + ["Contents"] {
            let next = openat(directory, component, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC)
            guard next >= 0 else { return unknown }
            close(directory)
            directory = next
        }
        let descriptor = openat(directory, "Info.plist", O_RDONLY | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC)
        guard descriptor >= 0 else { return unknown }
        defer { close(descriptor) }
        var before = stat()
        guard fstat(descriptor, &before) == 0, before.st_mode & S_IFMT == S_IFREG,
              before.st_flags & UInt32(0x40000000) == 0,
              before.st_size >= 0, before.st_size < 1_000_000 else { return unknown }
        let infoURL = root.appendingPathComponent("Contents/Info.plist")
        if let tree, tree[infoURL.path] != Identity(before) { return unknown }
        var data = Data(count: Int(before.st_size))
        let bytesRead = data.withUnsafeMutableBytes { buffer -> Int in
            var offset = 0
            while offset < buffer.count {
                let amount = read(descriptor, buffer.baseAddress!.advanced(by: offset), buffer.count - offset)
                if amount < 0 && errno == EINTR { continue }
                guard amount > 0 else { return -1 }
                offset += amount
            }
            return offset
        }
        var after = stat()
        guard bytesRead == data.count, fstat(descriptor, &after) == 0,
              Identity(before) == Identity(after), safeAncestors(root),
              let plist = try? PropertyListSerialization.propertyList(from: data, format: nil) as? [String: Any] else { return unknown }
        return (plist["CFBundleShortVersionString"] as? String ?? unknown.version, plist["CFBundleIdentifier"] as? String ?? "")
    }

    public func moveToTrash(ids: [String], runningApplicationPaths: Set<String> = [], runningBundleIdentifiers: Set<String> = [], liveRunningApplications: (@Sendable () async -> RunningApplications)? = nil, preTrashValidation: (@Sendable () async -> Bool)? = nil, finalValidation: (@Sendable () -> Bool)? = nil) async -> TrashReport {
        var successes: [String] = [], failures: [String] = []; var bytes: Int64 = 0
        let active = Set(runningApplicationPaths.map { URL(fileURLWithPath: $0).standardizedFileURL.path })
        for id in Set(ids) {
            guard !Task.isCancelled else { failures.append("Operazione interrotta."); break }
            guard let candidate = candidates[id], trashingIDs.insert(id).inserted else { failures.append("Elemento scaduto o non riconosciuto. Ripeti la scansione."); continue }
            defer { trashingIDs.remove(id) }
            let item = candidate.item
            guard inside(item.url.path, candidate.root.path), (item.url != candidate.root || item.category == .caches),
                  allowedRoot(candidate.root, category: item.category), candidate.rootIdentity.sameNode(as: try? identity(candidate.root)), !protected(item.url), safeAncestors(item.url), !placeholder(item.url),
                  (try? identity(item.url)) == candidate.identity,
                  !item.url.pathComponents.contains(where: { runningBundleIdentifiers.contains($0) }),
                  !active.contains(where: { inside($0, item.url.path) }), !inside(ownBundlePath, item.url.path),
                  item.category == .applications || !package(item.url) else {
                failures.append("Elemento modificato o protetto: \(item.title)"); continue
            }
            if item.category == .applications {
                let current = try? await readExecutor.run { [self] cancellation in
                    var budget = entryLimit
                    return try applicationTree(item.url, budget: &budget, check: cancellation.check)
                }
                let currentIdentifier = current.map { applicationMetadata(item.url, tree: $0).identifier } ?? ""
                guard let current, current == candidate.tree,
                      !currentIdentifier.isEmpty, currentIdentifier == candidate.appBundleIdentifier,
                      !currentIdentifier.hasPrefix("com.apple."), currentIdentifier != "com.piernicolademaria.lindo",
                      safeAncestors(item.url), candidate.rootIdentity.sameNode(as: try? identity(candidate.root)),
                      (try? identity(item.url)) == candidate.identity else {
                    failures.append("Applicazione modificata o protetta: \(item.title)"); continue
                }
            }
            if let preTrashValidation, !(await preTrashValidation()) {
                failures.append("Elemento modificato o protetto: \(item.title)"); continue
            }
            let live: RunningApplications?
            if let liveRunningApplications { live = await liveRunningApplications() } else { live = nil }
            let livePaths = Set((live?.paths ?? []).map { URL(fileURLWithPath: $0).standardizedFileURL.path })
            let allPaths = active.union(livePaths)
            let allIdentifiers = runningBundleIdentifiers.union(live?.bundleIdentifiers ?? [])
            if item.category == .applications {
                guard let live, live.inspectionComplete else {
                    failures.append("Impossibile verificare i processi di \(item.title). Verifica processi non completa."); continue
                }
            }
            if item.category == .caches {
                let cacheOwner = CachePolicy.owner(of: item.url, root: cacheRoot)
                guard let owner = cacheOwner,
                      let live, live.inspectionComplete,
                      !CachePolicy.isRunning(owner, paths: allPaths, identifiers: allIdentifiers) else {
                    failures.append("Chiudi \(cacheOwner?.title ?? item.title) prima di pulire la cache. Verifica processi non completa o browser in uso."); continue
                }
                var budget = entryLimit
                guard let tree = try? cacheTree(item.url, budget: &budget), tree == candidate.tree else {
                    failures.append("Cache modificata o non verificabile: \(item.title). Ripeti la scansione."); continue
                }
            }
            let bundleIdentifier = candidate.appBundleIdentifier
            // There must be no suspension between the final validation and trash.
            guard !Task.isCancelled, candidates[id] != nil, (finalValidation?() ?? true),
                  !item.url.pathComponents.contains(where: { allIdentifiers.contains($0) }),
                  !allPaths.contains(where: { inside($0, item.url.path) }),
                  bundleIdentifier.isEmpty || !allIdentifiers.contains(bundleIdentifier),
                  safeAncestors(item.url), !protected(item.url), !placeholder(item.url),
                  candidate.rootIdentity.sameNode(as: try? identity(candidate.root)),
                  (try? identity(item.url)) == candidate.identity,
                  candidate.tree.allSatisfy({ path, saved in !Task.isCancelled && (try? identity(URL(fileURLWithPath: path))) == saved }) else {
                failures.append("Elemento modificato, protetto o in uso: \(item.title)"); continue
            }
            if item.category == .applications {
                let finalProcesses = processInventory()
                guard finalProcesses.isComplete,
                      !finalProcesses.paths.contains(where: { inside(URL(fileURLWithPath: $0).standardizedFileURL.path, item.url.path) }) else {
                    failures.append("Chiudi \(item.title) prima di rimuoverla. Verifica processi finale non completa o applicazione in uso."); continue
                }
            }
            if item.category == .caches {
                let finalProcesses = processInventory()
                guard finalProcesses.isComplete,
                      let owner = CachePolicy.owner(of: item.url, root: cacheRoot),
                      !CachePolicy.isRunning(owner, paths: finalProcesses.paths, identifiers: []) else {
                    failures.append("Chiudi \(CachePolicy.owner(of: item.url, root: cacheRoot)?.title ?? item.title) prima di pulire la cache. Verifica processi finale non completa o browser in uso."); continue
                }
            }
            guard !Task.isCancelled else { failures.append("Operazione interrotta."); break }
            do { try trash(item.url); candidates.removeValue(forKey: id); successes.append(id); bytes += item.bytes }
            catch { failures.append("Impossibile spostare nel Cestino: \(item.title)") }
        }
        return TrashReport(trashedIDs: successes, failures: failures, bytes: bytes)
    }
}
