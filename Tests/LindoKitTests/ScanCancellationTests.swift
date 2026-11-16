import Foundation
import Testing
@testable import LindoKit

@Test func sequentialReadsReleaseCapacityBeforeReturning() async throws {
    let executor = ScanReadExecutor(limit: 1)
    // Each completed read must make room for the next one, including failures.
    // Repetition exercises the worker/caller handoff on different executors.
    for index in 0..<10_000 {
        let value = try await executor.run { _ in index }
        #expect(value == index)
        do {
            let _: Int = try await executor.run { _ in throw SequentialReadFailure.expected }
            Issue.record("Expected the read failure")
        } catch SequentialReadFailure.expected { }
    }
}

private enum SequentialReadFailure: Error { case expected }

private final class BlockedRead: @unchecked Sendable {
    private let lock = NSLock()
    private let gate = DispatchSemaphore(value: 0)
    private var entered = false
    private var exited = false
    var didEnter: Bool { lock.lock(); defer { lock.unlock() }; return entered }
    var didExit: Bool { lock.lock(); defer { lock.unlock() }; return exited }
    func block() {
        lock.lock(); entered = true; lock.unlock()
        // Watchdog only: successful cancellation must return before this read exits.
        _ = gate.wait(timeout: .now() + 10)
        lock.lock(); exited = true; lock.unlock()
    }
    func release() { gate.signal() }
}

@Test func scannerCancellationReturnsWhileFilesystemIsBlocked() async throws {
    let blocked = BlockedRead()
    defer { blocked.release() }
    let scanner = MacScanner(beforeScanRead: { blocked.block() }, trash: { _ in
        Issue.record("Read-only scan must never trash")
    })
    let task = Task { await scanner.scan(category: .largeFiles, roots: []) }
    defer { task.cancel() }
    let deadline = ContinuousClock.now.advanced(by: .seconds(5))
    while !blocked.didEnter && ContinuousClock.now < deadline { try await Task.sleep(for: .milliseconds(1)) }
    try #require(blocked.didEnter)
    task.cancel()
    let report = await task.value
    #expect(report.cancelled && report.isPartial)
    #expect(report.items.isEmpty)
    #expect(!blocked.didExit, "Cancellation waited for the blocked filesystem worker")
}

@Test func duplicateCancellationReturnsDuringContentRead() async throws {
    let fm = FileManager.default
    let root = fm.homeDirectoryForCurrentUser.appendingPathComponent("Library/Developer/Lindo/test-fixtures/cancel-" + UUID().uuidString)
    try fm.createDirectory(at: root, withIntermediateDirectories: true)
    defer { try? fm.removeItem(at: root) }
    try Data(repeating: 42, count: 4_096).write(to: root.appendingPathComponent("a.bin"))
    try Data(repeating: 42, count: 4_096).write(to: root.appendingPathComponent("b.bin"))
    let blocked = BlockedRead()
    defer { blocked.release() }
    let scanner = DuplicateScanner(beforeContentRead: { blocked.block() }, trash: { _ in Issue.record("Unexpected trash") })
    let task = Task { await scanner.scan(roots: [root]) }
    defer { task.cancel() }
    // A blocked content reader must not prevent cancellation or publish a group.
    let deadline = ContinuousClock.now.advanced(by: .seconds(5))
    while !blocked.didEnter && ContinuousClock.now < deadline { try await Task.sleep(for: .milliseconds(1)) }
    try #require(blocked.didEnter)
    task.cancel()
    let report = await task.value
    #expect(report.cancelled && report.isPartial && report.groups.isEmpty)
    #expect(!blocked.didExit, "Cancellation waited for the blocked content reader")
}

@Test func cancelledReaderKeepsItsSlotUntilWorkerExits() async throws {
    let executor = ScanReadExecutor(limit: 1)
    let blocked = BlockedRead()
    defer { blocked.release() }
    let task = Task {
        try await executor.run { cancellation in
            blocked.block()
            try cancellation.check()
            return 1
        }
    }
    defer { task.cancel() }
    let deadline = ContinuousClock.now.advanced(by: .seconds(5))
    while !blocked.didEnter && ContinuousClock.now < deadline { try await Task.sleep(for: .milliseconds(1)) }
    try #require(blocked.didEnter)
    task.cancel()
    do { _ = try await task.value; Issue.record("Expected cancellation") }
    catch is CancellationError { }
    #expect(!blocked.didExit, "Cancellation waited for the blocked filesystem worker")
    do {
        _ = try await executor.run { _ in 2 }
        Issue.record("Cancelled OS worker must retain its slot")
    } catch ScanReadError.busy { }
    blocked.release()
    // Let the worker release its slot; no new thread or queued backlog is created.
    let releaseDeadline = ContinuousClock.now.advanced(by: .seconds(5))
    while ContinuousClock.now < releaseDeadline {
        do { let value = try await executor.run { _ in 3 }; #expect(value == 3); return }
        catch ScanReadError.busy { try await Task.sleep(for: .milliseconds(1)) }
    }
    Issue.record("Reader did not release its slot after exit")
}
