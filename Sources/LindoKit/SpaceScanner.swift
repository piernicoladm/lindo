import Foundation
import Darwin

public struct SpaceEntry: Identifiable, Sendable {
    public let id: String
    public let url: URL
    public let allocatedBytes: Int64
    public let logicalBytes: Int64
    public let fileCount: Int
    public let isDirectory: Bool
    public let isPartial: Bool
    public let alreadyCountedLinks: Int

    public init(id: String? = nil, url: URL, allocatedBytes: Int64, logicalBytes: Int64, fileCount: Int, isDirectory: Bool, isPartial: Bool, alreadyCountedLinks: Int = 0) {
        self.id = id ?? url.path
        self.url = url
        self.allocatedBytes = allocatedBytes
        self.logicalBytes = logicalBytes
        self.fileCount = fileCount
        self.isDirectory = isDirectory
        self.isPartial = isPartial
        self.alreadyCountedLinks = alreadyCountedLinks
    }
}

public struct SpaceReport: Sendable {
    public let root: URL
    public let entries: [SpaceEntry]
    public let allocatedBytes: Int64
    public let logicalBytes: Int64
    public let fileCount: Int
    public let inspectedCount: Int
    public let warnings: [String]
    public let isPartial: Bool
    public let cancelled: Bool
    public let duration: TimeInterval

    public init(root: URL, entries: [SpaceEntry], allocatedBytes: Int64, logicalBytes: Int64, fileCount: Int, inspectedCount: Int, warnings: [String], isPartial: Bool, cancelled: Bool, duration: TimeInterval) {
        self.root = root
        self.entries = entries
        self.allocatedBytes = allocatedBytes
        self.logicalBytes = logicalBytes
        self.fileCount = fileCount
        self.inspectedCount = inspectedCount
        self.warnings = warnings
        self.isPartial = isPartial
        self.cancelled = cancelled
        self.duration = duration
    }
}

/// Read-only metadata accounting. No file contents or removal candidates exist.
public actor SpaceScanner {
    private let executor: ScanReadExecutor
    private let fixtureScopes: Bool
    private let beforeRead: (@Sendable () -> Void)?
    private var generation = UUID()
    private var pending: Task<SpaceReport, any Error>?

    public init() {
        executor = .shared
        fixtureScopes = false
        beforeRead = nil
    }

    init(executor: ScanReadExecutor, fixtureScopes: Bool = true, beforeRead: (@Sendable () -> Void)? = nil) {
        self.executor = executor
        self.fixtureScopes = fixtureScopes
        self.beforeRead = beforeRead
    }

    public func scan(root: URL) async -> SpaceReport {
        let root = root.standardizedFileURL
        let started = Date()
        pending?.cancel()
        let token = UUID()
        generation = token
        let executor = executor, fixtureScopes = fixtureScopes, beforeRead = beforeRead
        let task = Task {
            try await executor.run { cancellation in
                beforeRead?()
                try cancellation.check()
                return try SpaceWalker(root: root, fixtureScopes: fixtureScopes, cancellation: cancellation).run(started: started)
            }
        }
        pending = task
        do {
            let report = try await withTaskCancellationHandler {
                try await task.value
            } onCancel: { task.cancel() }
            guard generation == token, !Task.isCancelled else {
                return Self.empty(root, started: started, warning: "Mappa spazio interrotta.", cancelled: true)
            }
            pending = nil
            return report
        } catch {
            if generation == token { pending = nil }
            let cancelled = error is CancellationError || Task.isCancelled || generation != token
            return Self.empty(root, started: started, warning: cancelled ? "Mappa spazio interrotta." : "Lettura non disponibile: riprova dopo le altre scansioni.", cancelled: cancelled)
        }
    }

    private static func empty(_ root: URL, started: Date, warning: String, cancelled: Bool) -> SpaceReport {
        SpaceReport(root: root, entries: [], allocatedBytes: 0, logicalBytes: 0, fileCount: 0, inspectedCount: 0, warnings: [warning], isPartial: true, cancelled: cancelled, duration: Date().timeIntervalSince(started))
    }
}

private struct SpaceTotals {
    var allocated: Int64 = 0
    var logical: Int64 = 0
    var files = 0
    var alreadyCountedLinks = 0
    var partial = false
    mutating func add(_ other: SpaceTotals) {
        allocated += other.allocated
        logical += other.logical
        files += other.files
        alreadyCountedLinks += other.alreadyCountedLinks
        partial = partial || other.partial
    }
}

private struct SpaceNode: Hashable {
    let device: Int32
    let inode: UInt64
}

private final class SpaceWalker {
    private let root: URL
    private let fixtureScopes: Bool
    private let cancellation: ScanReadCancellation
    // Only multiply-linked regular files need remembered identities. No per-file
    // records or content buffers are retained during the walk.
    private var hardlinks: Set<SpaceNode> = []
    private var inspected = 0
    private var warnings: [String] = []
    private var omittedWarnings = 0
    private let warningLimit = 200
    private var entries: [SpaceEntry] = []

    init(root: URL, fixtureScopes: Bool, cancellation: ScanReadCancellation) {
        self.root = root
        self.fixtureScopes = fixtureScopes
        self.cancellation = cancellation
    }

    private func within(_ path: String, _ parent: String) -> Bool { path == parent || path.hasPrefix(parent + "/") }

    private func excluded(_ url: URL) -> Bool {
        let parts = url.pathComponents.map { $0.lowercased() }
        let forbidden: Set<String> = ["_6_archivio_private", "piernicola de maria", ".ssh", ".gnupg", ".aws", ".codex", "keychains", "mail", "messages", ".trash", ".trashes", "trash"]
        return parts.contains(where: forbidden.contains) || zip(parts, parts.dropFirst()).contains(where: { $0 == "_2_work" && $1 == "clients" })
    }

    private func warn(_ message: String) {
        if warnings.count < warningLimit { warnings.append(message) }
        else { omittedWarnings += 1 }
    }

    private func openRoot() -> Int32 {
        var descriptor = open("/", O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC)
        guard descriptor >= 0 else { return -1 }
        for component in root.pathComponents.dropFirst() {
            var expected = stat()
            guard fstatat(descriptor, component, &expected, AT_SYMLINK_NOFOLLOW) == 0,
                  expected.st_mode & S_IFMT == S_IFDIR,
                  expected.st_flags & UInt32(0x40000000) == 0 else { close(descriptor); return -1 }
            let next = openat(descriptor, component, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC)
            close(descriptor)
            guard next >= 0 else { return -1 }
            var actual = stat()
            guard fstat(next, &actual) == 0, actual.st_dev == expected.st_dev, actual.st_ino == expected.st_ino,
                  actual.st_flags & UInt32(0x40000000) == 0 else { close(next); return -1 }
            descriptor = next
        }
        return descriptor
    }

    func run(started: Date) throws -> SpaceReport {
        let home = FileManager.default.homeDirectoryForCurrentUser.path
        guard !excluded(root), !root.pathComponents.contains(where: { $0.hasSuffix(".icloud") }), fixtureScopes || [home, "/Applications", "/Volumes"].contains(where: { within(root.path, $0) }) else {
            warn("Cartella esclusa dalla mappa spazio: \(root.path)")
            return report(started: started)
        }
        try cancellation.check()
        let descriptor = openRoot()
        guard descriptor >= 0 else {
            warn("Cartella non accessibile o collegamento simbolico: \(root.path)")
            return report(started: started)
        }
        var info = stat()
        guard fstat(descriptor, &info) == 0, info.st_flags & UInt32(0x40000000) == 0 else {
            close(descriptor)
            warn("Cartella non disponibile localmente: \(root.path)")
            return report(started: started)
        }
        var ignoredRootTotals = SpaceTotals()
        try walk(descriptor, at: root, depth: 0, totals: &ignoredRootTotals)
        return report(started: started)
    }

    private func report(started: Date) -> SpaceReport {
        if omittedWarnings > 0 { warnings.append("Altre \(omittedWarnings) esclusioni o letture incomplete.") }
        let sorted = entries.sorted { $0.allocatedBytes == $1.allocatedBytes ? $0.url.path < $1.url.path : $0.allocatedBytes > $1.allocatedBytes }
        return SpaceReport(root: root, entries: sorted, allocatedBytes: sorted.reduce(0) { $0 + $1.allocatedBytes }, logicalBytes: sorted.reduce(0) { $0 + $1.logicalBytes }, fileCount: sorted.reduce(0) { $0 + $1.fileCount }, inspectedCount: inspected, warnings: warnings, isPartial: !warnings.isEmpty || sorted.contains(where: \.isPartial), cancelled: false, duration: Date().timeIntervalSince(started))
    }

    /// Consumes the directory descriptor, including on errors or cancellation.
    private func walk(_ descriptor: Int32, at directory: URL, depth: Int, totals: inout SpaceTotals) throws {
        guard let stream = fdopendir(descriptor) else {
            close(descriptor)
            totals.partial = true
            warn("Impossibile leggere la cartella: \(directory.path)")
            return
        }
        defer { closedir(stream) }
        while true {
            try cancellation.check()
            errno = 0
            guard let record = readdir(stream) else {
                if errno != 0 { totals.partial = true; warn("Lettura cartella incompleta: \(directory.path)") }
                break
            }
            let name = withUnsafePointer(to: &record.pointee.d_name) {
                $0.withMemoryRebound(to: CChar.self, capacity: Int(NAME_MAX) + 1) { String(cString: $0) }
            }
            if name == "." || name == ".." { continue }
            let url = directory.appendingPathComponent(name)
            if excluded(url) || name.hasSuffix(".icloud") {
                totals.partial = true
                warn("Elemento escluso dalla mappa spazio: \(url.path)")
                continue
            }
            var info = stat()
            inspected += 1
            guard fstatat(dirfd(stream), name, &info, AT_SYMLINK_NOFOLLOW) == 0 else {
                totals.partial = true; warn("Metadati non accessibili: \(url.path)"); continue
            }
            let kind = info.st_mode & S_IFMT
            guard kind == S_IFDIR || kind == S_IFREG, info.st_flags & UInt32(0x40000000) == 0 else {
                totals.partial = true; warn("Collegamento, elemento speciale o contenuto non locale escluso: \(url.path)"); continue
            }
            var child = SpaceTotals()
            if kind == S_IFREG && info.st_nlink > 1 && !hardlinks.insert(SpaceNode(device: info.st_dev, inode: info.st_ino)).inserted {
                // A second name contributes no additional physical or logical bytes.
                child.alreadyCountedLinks = 1
            } else {
                child.allocated = max(0, Int64(info.st_blocks)) * 512
                child.logical = max(0, Int64(info.st_size))
                child.files = kind == S_IFREG ? 1 : 0
            }
            if kind == S_IFDIR {
                if depth >= 64 {
                    child.partial = true; warn("Profondità massima raggiunta: \(url.path)")
                } else {
                    let next = openat(dirfd(stream), name, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC)
                    var opened = stat()
                    guard next >= 0 else {
                        child.partial = true; warn("Cartella non accessibile: \(url.path)")
                        totals.add(child)
                        if depth == 0 { append(url, info: info, totals: child) }
                        continue
                    }
                    guard fstat(next, &opened) == 0, opened.st_dev == info.st_dev, opened.st_ino == info.st_ino, opened.st_flags & UInt32(0x40000000) == 0 else {
                        close(next); child.partial = true; warn("Cartella modificata durante la lettura: \(url.path)")
                        totals.add(child)
                        if depth == 0 { append(url, info: info, totals: child) }
                        continue
                    }
                    try walk(next, at: url, depth: depth + 1, totals: &child)
                }
            }
            totals.add(child)
            if depth == 0 { append(url, info: info, totals: child) }
        }
    }

    private func append(_ url: URL, info: stat, totals: SpaceTotals) {
        entries.append(SpaceEntry(url: url, allocatedBytes: totals.allocated, logicalBytes: totals.logical, fileCount: totals.files, isDirectory: info.st_mode & S_IFMT == S_IFDIR, isPartial: totals.partial, alreadyCountedLinks: totals.alreadyCountedLinks))
    }
}
