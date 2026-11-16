import Foundation
import Darwin
import UniformTypeIdentifiers

public struct FolderEntry: Identifiable, Sendable {
    public let url: URL
    public let name: String
    public var id: String { url.path }
}

public struct FolderListing: Sendable {
    public let directory: URL
    public let children: [FolderEntry]
    public let warnings: [String]
    public let canSelect: Bool
    public let parent: URL?

    public init(directory: URL, children: [FolderEntry], warnings: [String], canSelect: Bool, parent: URL?) {
        self.directory = directory
        self.children = children
        self.warnings = warnings
        self.canSelect = canSelect
        self.parent = parent
    }
}

/// Keeps potentially blocking filesystem calls off Swift's cooperative executor.
public actor FolderDirectoryReader {
    private let home: URL
    private let entryLimit: Int
    private let executor: FolderReadExecutor
    private let beforeRead: (@Sendable () -> Void)?
    private let beforeEnumeration: (@Sendable () -> Void)?
    private let didReadEntry: (@Sendable (String) -> Void)?

    public init() {
        home = FileManager.default.homeDirectoryForCurrentUser.standardizedFileURL
        entryLimit = 2_000
        executor = .shared
        beforeRead = nil
        beforeEnumeration = nil
        didReadEntry = nil
    }

    internal init(home: URL, entryLimit: Int = 2_000,
                  executor: FolderReadExecutor = .shared, beforeRead: (@Sendable () -> Void)? = nil,
                  beforeEnumeration: (@Sendable () -> Void)? = nil,
                  didReadEntry: (@Sendable (String) -> Void)? = nil) {
        self.home = home.standardizedFileURL
        self.entryLimit = max(1, entryLimit)
        self.executor = executor
        self.beforeRead = beforeRead
        self.beforeEnumeration = beforeEnumeration
        self.didReadEntry = didReadEntry
    }

    public nonisolated func list(_ url: URL) async throws -> FolderListing {
        try Task.checkCancellation()
        let request = FolderReadRequest()
        let worker = FolderDirectoryWorker(home: home, entryLimit: entryLimit, request: request, beforeEnumeration: beforeEnumeration, didReadEntry: didReadEntry)
        return try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { continuation in
                guard request.install(continuation) else { return }
                let submitted = executor.submit { [beforeRead] in
                    do {
                        try request.checkCancellation()
                        beforeRead?()
                        let listing = try worker.list(url)
                        request.finish(.success(listing))
                    } catch {
                        request.finish(.failure(error))
                    }
                }
                if !submitted { request.finish(.failure(FolderReadError.busy)) }
            }
        } onCancel: {
            request.cancel()
        }
    }
}

/// Slots include queued work and remain occupied until the filesystem worker actually exits.
internal final class FolderReadExecutor: @unchecked Sendable {
    static let shared = FolderReadExecutor()
    private let lock = NSLock()
    private var running = 0
    private let limit = 2
    private let queue = DispatchQueue(label: "it.imprendilife.lindo.folder-reader", qos: .userInitiated, attributes: .concurrent)

    func submit(_ operation: @escaping @Sendable () -> Void) -> Bool {
        lock.lock()
        guard running < limit else { lock.unlock(); return false }
        running += 1
        lock.unlock()
        queue.async {
            defer {
                self.lock.lock()
                self.running -= 1
                self.lock.unlock()
            }
            operation()
        }
        return true
    }
}

/// Cancellation resumes the caller immediately; an in-flight syscall keeps its slot.
private final class FolderReadRequest: @unchecked Sendable {
    private let lock = NSLock()
    private var cancelled = false
    private var continuation: CheckedContinuation<FolderListing, any Error>?

    func install(_ continuation: CheckedContinuation<FolderListing, any Error>) -> Bool {
        lock.lock()
        guard !cancelled else {
            lock.unlock()
            continuation.resume(throwing: CancellationError())
            return false
        }
        self.continuation = continuation
        lock.unlock()
        return true
    }

    func checkCancellation() throws {
        lock.lock()
        let value = cancelled
        lock.unlock()
        if value { throw CancellationError() }
    }

    func cancel() {
        lock.lock()
        cancelled = true
        let pending = continuation
        continuation = nil
        lock.unlock()
        pending?.resume(throwing: CancellationError())
    }

    func finish(_ result: Result<FolderListing, any Error>) {
        lock.lock()
        let pending = continuation
        continuation = nil
        lock.unlock()
        pending?.resume(with: result)
    }
}

private struct FolderDirectoryWorker: Sendable {
    let home: URL
    let entryLimit: Int
    let request: FolderReadRequest
    let beforeEnumeration: (@Sendable () -> Void)?
    let didReadEntry: (@Sendable (String) -> Void)?

    private func within(_ path: String, _ root: String) -> Bool {
        path == root || path.hasPrefix(root + "/")
    }

    private func permits(_ url: URL) -> Bool {
        let path = url.path
        let parts = url.pathComponents.dropFirst().map { $0.lowercased() }
        guard !parts.contains(where: { $0.hasPrefix(".") }), !ScanPathPolicy.isProtected(url) else { return false }
        // The Library rule folds letter case: "library" names the same folder
        // on the default volume and must not become an ordinary home folder.
        let folded = path.lowercased(), library = (home.path + "/Library").lowercased()
        if within(folded, library) {
            return within(folded, library + "/cloudstorage") || within(folded, library + "/mobile documents")
        }
        return within(path, home.path) || within(path, "/Volumes")
    }

    /// Reads Finder flags from the open directory, never a mutable pathname.
    private func metadata(_ descriptor: Int32, at url: URL, leaf: Bool) throws -> Bool {
        try request.checkCancellation()
        var info = stat()
        guard fstat(descriptor, &info) == 0, info.st_mode & S_IFMT == S_IFDIR,
              info.st_flags & UInt32(0x40000000) == 0,
              !ScanPathPolicy.packages.contains(url.pathExtension.lowercased()),
              UTType(filenameExtension: url.pathExtension)?.conforms(to: .package) != true,
              !url.lastPathComponent.hasSuffix(".icloud") else { return false }
        if leaf && (info.st_mode & 0o444 == 0 || info.st_mode & 0o111 == 0) { return false }
        var attributes = attrlist()
        attributes.bitmapcount = UInt16(ATTR_BIT_MAP_COUNT)
        attributes.commonattr = attrgroup_t(ATTR_CMN_FNDRINFO)
        // Four-byte returned size followed by the 32-byte FinderInfo record.
        var buffer = [UInt8](repeating: 0, count: 36)
        let result = buffer.withUnsafeMutableBytes {
            fgetattrlist(descriptor, &attributes, $0.baseAddress, $0.count, 0)
        }
        guard result == 0 else { return false }
        let finderFlags = UInt16(buffer[12]) << 8 | UInt16(buffer[13])
        guard finderFlags & 0x2000 == 0 else { return false } // kHasBundle
        let checksHidden = within(url.path, home.path) || within(url.path, "/Volumes")
        if checksHidden && url.path != home.path + "/Library",
           info.st_flags & UInt32(UF_HIDDEN) != 0 || finderFlags & 0x4000 != 0 { return false }
        return true
    }

    private func mountedScope(_ descriptor: Int32, at url: URL) -> Bool {
        guard within(url.path, "/Volumes"), url.path != "/Volumes" else { return true }
        let components = url.pathComponents
        guard components.count >= 3 else { return false }
        var volume = statfs()
        guard fstatfs(descriptor, &volume) == 0 else { return false }
        let mountPath = withUnsafePointer(to: &volume.f_mntonname) {
            $0.withMemoryRebound(to: CChar.self, capacity: Int(MAXPATHLEN)) { String(cString: $0) }
        }
        return mountPath == "/Volumes/" + components[2]
    }

    /// Opens each component relative to its pinned parent, refusing symlinks.
    private func openDirectory(_ url: URL) throws -> Int32 {
        var descriptor = open("/", O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC)
        guard descriptor >= 0 else { throw FolderReadError.unavailable }
        do {
            var cursor = URL(fileURLWithPath: "/", isDirectory: true)
            for component in url.pathComponents.dropFirst() {
                try request.checkCancellation()
                cursor.appendPathComponent(component, isDirectory: true)
                var expected = stat()
                guard fstatat(descriptor, component, &expected, AT_SYMLINK_NOFOLLOW) == 0,
                      expected.st_mode & S_IFMT == S_IFDIR,
                      expected.st_flags & UInt32(0x40000000) == 0 else { throw FolderReadError.unavailable }
                let next = openat(descriptor, component, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC)
                guard next >= 0 else { throw FolderReadError.unavailable }
                close(descriptor)
                descriptor = next
                var actual = stat()
                guard fstat(descriptor, &actual) == 0,
                      actual.st_dev == expected.st_dev, actual.st_ino == expected.st_ino,
                      try metadata(descriptor, at: cursor, leaf: cursor.path == url.path) else { throw FolderReadError.unavailable }
            }
            guard mountedScope(descriptor, at: url) else { throw FolderReadError.unavailable }
            return descriptor
        } catch {
            close(descriptor)
            throw error
        }
    }

    private func validatePath(_ url: URL, descriptor: Int32) throws {
        let current = try openDirectory(url)
        defer { close(current) }
        var expected = stat()
        var actual = stat()
        guard fstat(descriptor, &expected) == 0, fstat(current, &actual) == 0,
              expected.st_dev == actual.st_dev, expected.st_ino == actual.st_ino else { throw FolderReadError.unavailable }
    }

    func list(_ url: URL) throws -> FolderListing {
        try request.checkCancellation()
        guard url.isFileURL else { throw FolderReadError.unavailable }
        let directory = url.standardizedFileURL
        guard permits(directory) else { throw FolderReadError.unavailable }
        let descriptor = try openDirectory(directory)
        guard let stream = fdopendir(descriptor) else {
            close(descriptor)
            throw FolderReadError.unavailable
        }
        defer { closedir(stream) }
        beforeEnumeration?()
        try validatePath(directory, descriptor: descriptor)
        var children: [FolderEntry] = []
        var warnings: [String] = []
        var count = 0
        while true {
            try request.checkCancellation()
            if count >= entryLimit {
                warnings.append("Sono state esaminate solo le prime \(entryLimit) voci. La cartella potrebbe contenere altre sottocartelle.")
                break
            }
            errno = 0
            guard let record = readdir(stream) else {
                if errno != 0 { throw FolderReadError.unavailable }
                break
            }
            let name = withUnsafePointer(to: &record.pointee.d_name) {
                $0.withMemoryRebound(to: CChar.self, capacity: Int(NAME_MAX) + 1) { String(cString: $0) }
            }
            didReadEntry?(name)
            if name == "." || name == ".." { continue }
            count += 1
            if name.hasPrefix(".") { continue }
            let child = directory.appendingPathComponent(name, isDirectory: true)
            if ScanPathPolicy.isProtected(child) {
                warnings.append("Cartella esclusa dalle protezioni: \(name)")
                continue
            }
            guard permits(child) else { continue }
            var expected = stat()
            guard fstatat(descriptor, name, &expected, AT_SYMLINK_NOFOLLOW) == 0,
                  expected.st_mode & S_IFMT == S_IFDIR,
                  expected.st_flags & UInt32(0x40000000) == 0 else { continue }
            let childDescriptor = openat(descriptor, name, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC)
            guard childDescriptor >= 0 else { continue }
            defer { close(childDescriptor) }
            var actual = stat()
            guard fstat(childDescriptor, &actual) == 0,
                  actual.st_dev == expected.st_dev, actual.st_ino == expected.st_ino,
                  try metadata(childDescriptor, at: child, leaf: true),
                  mountedScope(childDescriptor, at: child) else { continue }
            children.append(FolderEntry(url: child, name: name))
        }
        try request.checkCancellation()
        try validatePath(directory, descriptor: descriptor)
        children.sort { $0.name.localizedStandardCompare($1.name) == .orderedAscending }
        let parent = directory.deletingLastPathComponent()
        var canGoUp = false
        if directory.path != home.path && directory.path != "/Volumes", permits(parent) {
            if let parentDescriptor = try? openDirectory(parent) {
                close(parentDescriptor)
                canGoUp = true
            }
        }
        try request.checkCancellation()
        return FolderListing(directory: directory, children: children, warnings: warnings,
            canSelect: directory.path != home.path && directory.path != "/Volumes", parent: canGoUp ? parent : nil)
    }

}

private enum FolderReadError: LocalizedError {
    case unavailable
    case busy
    var errorDescription: String? {
        switch self {
        case .unavailable:
            return "Questa cartella non è accessibile o non è consentita. Scegli una cartella personale accessibile, esclusi archivi privati, collegamenti e pacchetti."
        case .busy:
            return "Due letture di cartelle sono ancora in attesa del sistema. Attendi oppure riapri Lindo."
        }
    }
}
