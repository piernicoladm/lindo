import Foundation
import Testing
@testable import LindoKit

private final class ScanTestSignal: @unchecked Sendable {
    private let lock = NSLock()
    private var signalled = false
    private var waiters: [UUID: CheckedContinuation<Bool, Never>] = [:]

    var isSignalled: Bool { lock.withLock { signalled } }

    func signal() {
        let pending = lock.withLock {
            signalled = true
            let pending = Array(waiters.values)
            waiters.removeAll()
            return pending
        }
        for continuation in pending { continuation.resume(returning: true) }
    }

    func wait() async -> Bool {
        let id = UUID()
        return await withCheckedContinuation { continuation in
            let ready = lock.withLock {
                if signalled { return true }
                waiters[id] = continuation
                return false
            }
            if ready { continuation.resume(returning: true); return }
            DispatchQueue.global().asyncAfter(deadline: .now() + 2) { [self] in
                let pending = lock.withLock { waiters.removeValue(forKey: id) }
                pending?.resume(returning: false)
            }
        }
    }
}

private final class FirstScanReadGate: @unchecked Sendable {
    private let lock = NSLock()
    private var claimed = false
    private let semaphore = DispatchSemaphore(value: 0)
    let entered = ScanTestSignal()
    let exited = ScanTestSignal()

    func beforeRead() {
        let first = lock.withLock {
            if claimed { return false }
            claimed = true
            return true
        }
        guard first else { return }
        entered.signal()
        _ = semaphore.wait(timeout: .now() + 2)
        exited.signal()
    }

    func release() { semaphore.signal() }
}

private final class ConcurrentTrashRecorder: @unchecked Sendable {
    private let lock = NSLock()
    private var urls: [URL] = []
    func add(_ url: URL) { lock.withLock { urls.append(url) } }
    var count: Int { lock.withLock { urls.count } }
}

private struct ScanConcurrencyFixture {
    let root: URL
    init() throws {
        root = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(
            "Library/Developer/Lindo/test-fixtures/concurrency-" + UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        try Data("fixture bytes".utf8).write(to: root.appendingPathComponent("item.bin"))
    }
    func clean() { try? FileManager.default.removeItem(at: root) }
}

@Test func supersededBlockedScanCannotReplaceNewCandidates() async throws {
    let fixture = try ScanConcurrencyFixture(); defer { fixture.clean() }
    let gate = FirstScanReadGate(); defer { gate.release() }
    let recorder = ConcurrentTrashRecorder()
    let scanner = MacScanner(beforeScanRead: { gate.beforeRead() }, readExecutor: ScanReadExecutor(limit: 2),
                             trash: { recorder.add($0) })
    let oldTask = Task { await scanner.scan(category: .largeFiles, roots: [fixture.root], minimumBytes: 1) }
    defer { oldTask.cancel() }
    let entered = await gate.entered.wait()
    try #require(entered, "First reader never entered the controlled gate")
    let current = await scanner.scan(category: .largeFiles, roots: [fixture.root], minimumBytes: 1)
    #expect(!current.cancelled && !current.isPartial)
    let item = try #require(current.items.first)
    let old = await oldTask.value
    #expect(old.cancelled && old.isPartial && old.items.isEmpty)
    #expect(!gate.exited.isSignalled, "Supersession waited for the blocked reader")
    gate.release()
    let exited = await gate.exited.wait()
    try #require(exited, "Old reader did not exit after release")
    let result = await scanner.moveToTrash(ids: [item.id])
    #expect(result.trashedIDs == [item.id] && result.failures.isEmpty)
    #expect(recorder.count == 1)
}

@Test func concurrentTrashOfSameIDOnlyInvokesTrashOnce() async throws {
    let fixture = try ScanConcurrencyFixture(); defer { fixture.clean() }
    let entered = ScanTestSignal(), release = ScanTestSignal()
    defer { release.signal() }
    let recorder = ConcurrentTrashRecorder()
    let scanner = MacScanner(trash: { recorder.add($0) })
    let scan = await scanner.scan(category: .largeFiles, roots: [fixture.root], minimumBytes: 1)
    let item = try #require(scan.items.first)
    let firstTask = Task {
        await scanner.moveToTrash(ids: [item.id], preTrashValidation: {
            entered.signal()
            return await release.wait()
        })
    }
    defer { firstTask.cancel() }
    let waiting = await entered.wait()
    try #require(waiting, "First trash request never reached validation")
    let second = await scanner.moveToTrash(ids: [item.id])
    #expect(second.trashedIDs.isEmpty && second.failures.count == 1)
    #expect(recorder.count == 0)
    release.signal()
    let first = await firstTask.value
    #expect(first.trashedIDs == [item.id] && first.failures.isEmpty)
    #expect(recorder.count == 1)
}
