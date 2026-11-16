import Foundation
import CryptoKit
import Darwin

public struct DuplicateGroup: Identifiable, Sendable {
    public let id: String
    public let files: [ScanItem]
    public let keeperID: String
    public let digest: String
    public init(id: String = UUID().uuidString, files: [ScanItem], keeperID: String, digest: String) {
        self.id = id; self.files = files; self.keeperID = keeperID; self.digest = digest
    }
}

public struct DuplicateReport: Sendable {
    public let groups: [DuplicateGroup]
    public let warnings: [String]
    public let inspectedCount: Int
    public let isPartial: Bool
    public let cancelled: Bool
    public let duration: TimeInterval
}

private struct DuplicateIdentity: Equatable, Sendable {
    let device: Int32
    let inode: UInt64
    let mode: UInt16
    let size: Int64
    let modified: Int
    let modifiedNS: Int
    let changed: Int
    let changedNS: Int
    let flags: UInt32
    init(_ value: stat) {
        device = value.st_dev; inode = value.st_ino; mode = value.st_mode; size = value.st_size
        modified = value.st_mtimespec.tv_sec; modifiedNS = value.st_mtimespec.tv_nsec
        changed = value.st_ctimespec.tv_sec; changedNS = value.st_ctimespec.tv_nsec; flags = value.st_flags
    }
    func sameNode(_ other: Self) -> Bool { device == other.device && inode == other.inode && mode == other.mode }
}

private struct DuplicateSnapshot: Sendable {
    let item: ScanItem
    let identity: DuplicateIdentity
    let ancestors: [String: DuplicateIdentity]
}

private enum DuplicateError: Error { case changed, interrupted, unreadable, resourceFork }

private final class DuplicateGeneration: @unchecked Sendable {
    private let lock = NSLock()
    private var token = UUID()
    func set(_ value: UUID) { lock.lock(); defer { lock.unlock() }; token = value }
    func matches(_ value: UUID) -> Bool { lock.lock(); defer { lock.unlock() }; return token == value }
}

public actor DuplicateScanner {
    private let base: MacScanner
    private let gate = DuplicateGeneration()
    private var generation = UUID()
    private var groups: [String: DuplicateGroup] = [:]
    private var snapshots: [String: DuplicateSnapshot] = [:]
    private var trashInProgress = false
    private let readExecutor: ScanReadExecutor
    private let beforeContentRead: (@Sendable () -> Void)?
    public init() { base = MacScanner(); beforeContentRead = nil; readExecutor = .shared }
    init(scanner: MacScanner) { base = scanner; beforeContentRead = nil; readExecutor = ScanReadExecutor() }
    init(entryLimit: Int = Int.max, fixtureScopes: Bool = true, beforeContentRead: (@Sendable () -> Void)? = nil, readExecutor: ScanReadExecutor = ScanReadExecutor(), trash: @escaping @Sendable (URL) throws -> Void) {
        self.readExecutor = readExecutor
        self.beforeContentRead = beforeContentRead
        base = MacScanner(entryLimit: entryLimit, fixtureScopes: fixtureScopes, readExecutor: readExecutor, trash: trash)
    }

    private nonisolated func identity(_ url: URL) throws -> DuplicateIdentity {
        var value = stat()
        guard lstat(url.path, &value) == 0 else { throw DuplicateError.unreadable }
        return DuplicateIdentity(value)
    }
    private nonisolated func local(_ url: URL, _ value: DuplicateIdentity) -> Bool {
        guard value.flags & 0x40000000 == 0, !url.lastPathComponent.hasSuffix(".icloud"),
              let resource = try? url.resourceValues(forKeys: [.isUbiquitousItemKey, .ubiquitousItemDownloadingStatusKey]) else { return false }
        return resource.isUbiquitousItem != true || resource.ubiquitousItemDownloadingStatus == .current
    }
    private nonisolated func requireNoResourceFork(_ url: URL) throws {
        // Query only the attribute size, without allocating or reading fork content.
        // Reject symlinks in every component, including a concurrently replaced parent.
        let size = getxattr(url.path, "com.apple.ResourceFork", nil, 0, 0, XATTR_NOFOLLOW | XATTR_NOFOLLOW_ANY)
        if size > 0 { throw DuplicateError.resourceFork }
        guard size == 0 || (size == -1 && errno == ENOATTR) else { throw DuplicateError.unreadable }
    }
    private nonisolated func snapshot(_ item: ScanItem) throws -> DuplicateSnapshot {
        let value = try identity(item.url)
        guard value.mode & S_IFMT == S_IFREG, value.size == item.bytes, local(item.url, value) else { throw DuplicateError.changed }
        try requireNoResourceFork(item.url)
        var ancestors: [String: DuplicateIdentity] = [:]
        var parent = item.url.deletingLastPathComponent()
        while true {
            let info = try identity(parent)
            guard info.mode & S_IFMT == S_IFDIR, local(parent, info) else { throw DuplicateError.changed }
            ancestors[parent.path] = info
            if parent.path == "/" { break }
            parent.deleteLastPathComponent()
        }
        return DuplicateSnapshot(item: item, identity: value, ancestors: ancestors)
    }
    private nonisolated func validate(_ saved: DuplicateSnapshot) throws {
        guard try identity(saved.item.url) == saved.identity, local(saved.item.url, saved.identity) else { throw DuplicateError.changed }
        try requireNoResourceFork(saved.item.url)
        for (path, value) in saved.ancestors {
            let url = URL(fileURLWithPath: path), current = try identity(URL(fileURLWithPath: path))
            guard value.sameNode(current), current.mode & S_IFMT == S_IFDIR, local(url, current) else { throw DuplicateError.changed }
        }
    }
    /// After a move, only presence matters: the same regular file with the same length.
    /// Timestamps are ignored so a sync service touching the retained copy cannot raise a false alarm.
    private nonisolated func stillPresent(_ saved: DuplicateSnapshot) -> Bool {
        guard let current = try? identity(saved.item.url) else { return false }
        return saved.identity.sameNode(current) && current.mode & S_IFMT == S_IFREG && current.size == saved.identity.size
    }
    private nonisolated func openChecked(_ saved: DuplicateSnapshot) throws -> Int32 {
        try validate(saved)
        let fd = open(saved.item.url.path, O_RDONLY | O_NOFOLLOW | O_NONBLOCK)
        guard fd >= 0 else { throw DuplicateError.unreadable }
        do { try validateFD(fd, saved); return fd } catch { close(fd); throw error }
    }
    private nonisolated func validateFD(_ fd: Int32, _ saved: DuplicateSnapshot) throws {
        var value = stat()
        guard fstat(fd, &value) == 0, DuplicateIdentity(value) == saved.identity else { throw DuplicateError.changed }
        try validate(saved)
    }
    private func checkpoint(_ token: UUID) throws {
        guard !Task.isCancelled, generation == token else { throw DuplicateError.interrupted }
    }
    private nonisolated func block(_ fd: Int32) throws -> Data {
        var data = Data(count: 1_048_576)
        let count = data.withUnsafeMutableBytes { Darwin.read(fd, $0.baseAddress!, $0.count) }
        guard count >= 0 else { throw DuplicateError.unreadable }
        data.count = count
        return data
    }
    private nonisolated func digest(_ saved: DuplicateSnapshot, check: () throws -> Void) throws -> String {
        let fd = try openChecked(saved); defer { close(fd) }
        var hasher = SHA256(), bytes: Int64 = 0
        while true {
            try check()
            let data = try block(fd)
            if data.isEmpty { break }
            bytes += Int64(data.count)
            guard bytes <= saved.identity.size else { throw DuplicateError.changed }
            hasher.update(data: data)
        }
        try check(); try validateFD(fd, saved)
        guard bytes == saved.identity.size else { throw DuplicateError.changed }
        return hasher.finalize().map { String(format: "%02x", $0) }.joined()
    }
    private nonisolated func equalContents(_ left: DuplicateSnapshot, _ right: DuplicateSnapshot, check: () throws -> Void) throws -> Bool {
        guard !left.identity.sameNode(right.identity), left.identity.size == right.identity.size else { return false }
        let a = try openChecked(left); defer { close(a) }
        let b = try openChecked(right); defer { close(b) }
        var bytes: Int64 = 0, matches = true
        while true {
            try check()
            let x = try block(a), y = try block(b)
            if x != y { matches = false; break }
            if x.isEmpty { break }
            bytes += Int64(x.count)
            guard bytes <= left.identity.size else { throw DuplicateError.changed }
        }
        try check(); try validateFD(a, left); try validateFD(b, right)
        return matches && bytes == left.identity.size
    }
    private func read<Value: Sendable>(_ token: UUID, operation: @escaping @Sendable (@Sendable () throws -> Void) throws -> Value) async throws -> Value {
        try checkpoint(token)
        do {
            let value = try await readExecutor.run { [self] cancellation in
                let check: @Sendable () throws -> Void = {
                    try cancellation.check()
                    guard self.gate.matches(token) else { throw DuplicateError.interrupted }
                }
                try check()
                let value = try operation(check)
                try check()
                return value
            }
            try checkpoint(token)
            return value
        } catch is CancellationError { throw DuplicateError.interrupted }
    }

    private func fingerprint(_ item: ScanItem, token: UUID) async throws -> (DuplicateSnapshot, String) {
        try await read(token) { [self] check in
            beforeContentRead?()
            try check()
            let value = try snapshot(item)
            return (value, try digest(value, check: check))
        }
    }

    private func equal(_ left: DuplicateSnapshot, _ right: DuplicateSnapshot, token: UUID) async throws -> Bool {
        try await read(token) { [self] check in try equalContents(left, right, check: check) }
    }


    public func scan(roots: [URL], minimumBytes: Int64 = 1, progress: (@Sendable (String) async -> Void)? = nil) async -> DuplicateReport {
        let started = Date(), token = UUID()
        generation = token; gate.set(token); groups = [:]; snapshots = [:]
        let report = await base.scan(category: .largeFiles, roots: roots, minimumBytes: max(1, minimumBytes))
        var warnings = report.warnings, found: [DuplicateGroup] = [], saved: [String: DuplicateSnapshot] = [:]
        var cancelled = report.cancelled
        do {
            try checkpoint(token)
            let sizes = Dictionary(grouping: report.items, by: \.bytes).values.filter { $0.count > 1 }
            for bucket in sizes {
                var hashes: [String: [DuplicateSnapshot]] = [:]
                for item in bucket.sorted(by: { $0.url.path < $1.url.path }) {
                    try checkpoint(token)
                    if let progress { await progress("Verifica: \(item.title)"); try checkpoint(token) }
                    do {
                        let (value, hash) = try await fingerprint(item, token: token)
                        hashes[hash, default: []].append(value)
                    } catch DuplicateError.interrupted { throw DuplicateError.interrupted }
                    catch DuplicateError.resourceFork { warnings.append("File escluso: \(item.title). Contiene un resource fork non confrontato.") }
                    catch ScanReadError.busy { throw ScanReadError.busy }
                    catch { warnings.append("File modificato o non verificabile: \(item.title)") }
                }
                for (hash, values) in hashes where values.count > 1 {
                    var partitions: [[DuplicateSnapshot]] = []
                    for value in values {
                        var matched = false
                        for index in partitions.indices {
                            if try await equal(partitions[index][0], value, token: token) {
                                partitions[index].append(value); matched = true; break
                            }
                        }
                        if !matched { partitions.append([value]) }
                    }
                    for partition in partitions where partition.count > 1 {
                        try await read(token) { [self, partition] check in
                            for value in partition { try check(); try validate(value) }
                        }
                        let group = DuplicateGroup(files: partition.map(\.item), keeperID: partition[0].item.id, digest: hash)
                        found.append(group)
                        for value in partition { saved[value.item.id] = value }
                    }
                }
            }
            try checkpoint(token)
        } catch DuplicateError.interrupted { cancelled = true; warnings.append("Ricerca duplicati interrotta.") }
        catch ScanReadError.busy { warnings.append("Letture precedenti ancora in attesa del sistema. Riprova o scegli una cartella locale.") }
        catch { warnings.append("Confronto interrotto: file modificato o non accessibile.") }
        if generation == token && !cancelled {
            groups = Dictionary(uniqueKeysWithValues: found.map { ($0.id, $0) }); snapshots = saved
        } else { found = [] }
        return DuplicateReport(groups: found.sorted { $0.files[0].url.path < $1.files[0].url.path }, warnings: warnings, inspectedCount: report.inspectedCount, isPartial: report.isPartial || !warnings.isEmpty || cancelled, cancelled: cancelled, duration: Date().timeIntervalSince(started))
    }

    private func validPair(_ candidate: DuplicateSnapshot, keeper: DuplicateSnapshot, token: UUID) async -> Bool {
        do { return try await equal(candidate, keeper, token: token) } catch { return false }
    }

    public func moveToTrash(ids: [String], keepers: [String: String]) async -> TrashReport {
        guard !trashInProgress else { return TrashReport(trashedIDs: [], failures: ["Operazione già in corso."], bytes: 0) }
        trashInProgress = true; defer { trashInProgress = false }
        let token = generation, selected = Set(ids)
        var successes: [String] = [], failures: [String] = [], bytes: Int64 = 0
        // Freeze the reviewed group membership before any suspension.
        let reviewed = groups, recorded = snapshots
        var membership: [String: String] = [:]
        var groupMembers: [String: Set<String>] = [:]
        for group in reviewed.values {
            let members = Set(group.files.map(\.id))
            groupMembers[group.id] = members
            for id in members { membership[id] = group.id }
        }
        for id in selected.sorted() {
            guard generation == token, !Task.isCancelled else { failures.append("Operazione interrotta. Ripeti la scansione."); break }
            guard let groupID = membership[id], let keeperID = keepers[groupID], !selected.contains(keeperID),
                  groupMembers[groupID]?.contains(keeperID) == true,
                  let keeper = recorded[keeperID], let candidate = recorded[id], snapshots[id] != nil else {
                failures.append("Duplicato o copia da conservare non più valido. Ripeti la scansione."); continue
            }
            do {
                try await read(token) { [self] check in
                    try check(); try requireNoResourceFork(candidate.item.url)
                    try check(); try requireNoResourceFork(keeper.item.url)
                }
            } catch DuplicateError.resourceFork {
                failures.append("Duplicato o copia da conservare contiene un resource fork non confrontato. Ripeti la scansione."); continue
            } catch {
                failures.append("Duplicato o copia da conservare non verificabile. Ripeti la scansione."); continue
            }
            guard await validPair(candidate, keeper: keeper, token: token) else {
                failures.append("Duplicato o copia da conservare non più valido. Ripeti la scansione."); continue
            }
            guard generation == token else { failures.append("Selezione scaduta."); break }
            // Recheck content after entering the base actor, then identities without suspension.
            let result = await base.moveToTrash(ids: [id], preTrashValidation: { [self] in
                await self.validPair(candidate, keeper: keeper, token: token)
            }, finalValidation: { [self] in
                guard gate.matches(token), !Task.isCancelled else { return false }
                do { try validate(keeper); try validate(candidate); return true } catch { return false }
            })
            if generation == token, result.trashedIDs.contains(id) { snapshots.removeValue(forKey: id) }
            successes.append(contentsOf: result.trashedIDs); failures.append(contentsOf: result.failures); bytes += result.bytes
            // The last check and the move cannot be atomic. Confirm afterwards that the
            // retained copy survived; otherwise say where the removed copy is and stop.
            if result.trashedIDs.contains(id), !stillPresent(keeper) {
                failures.append("Attenzione: la copia da conservare di \(candidate.item.title) non è più verificabile. La copia spostata è nel Cestino: ripristinala dal Finder prima di svuotarlo. Operazione interrotta.")
                break
            }
        }
        return TrashReport(trashedIDs: successes, failures: failures, bytes: bytes)
    }
}
