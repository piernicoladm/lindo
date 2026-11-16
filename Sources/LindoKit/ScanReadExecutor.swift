import Foundation

internal enum ScanReadError: Error { case busy }

/// Bounded, read-only filesystem work. Cancelling a caller never frees a slot
/// until its actual OS operation exits. Never submit a mutation to this pool.
internal final class ScanReadExecutor: @unchecked Sendable {
    static let shared = ScanReadExecutor()
    private let lock = NSLock()
    private var active = 0
    private let limit: Int
    private let queue = DispatchQueue(label: "it.imprendilife.lindo.scan-reader", qos: .userInitiated, attributes: .concurrent)

    init(limit: Int = 2) { self.limit = max(1, limit) }

    func run<Value: Sendable>(_ operation: @escaping @Sendable (ScanReadCancellation) throws -> Value) async throws -> Value {
        let request = ScanReadRequest<Value>()
        return try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { continuation in
                guard request.install(continuation) else { return }
                let submitted = submit {
                    try request.cancellation.check()
                    return try operation(request.cancellation)
                } completion: { result in request.finish(result) }
                if !submitted { request.finish(.failure(ScanReadError.busy)) }
            }
        } onCancel: { request.cancel() }
    }

    private func submit<Value: Sendable>(_ operation: @escaping @Sendable () throws -> Value,
                                        completion: @escaping @Sendable (Result<Value, any Error>) -> Void) -> Bool {
        lock.lock()
        guard active < limit else { lock.unlock(); return false }
        active += 1
        lock.unlock()
        queue.async {
            let result = Result { try operation() }
            // Release only after the OS work exits, but before resuming a caller
            // that may immediately submit its next sequential read.
            self.lock.withLock { self.active -= 1 }
            completion(result)
        }
        return true
    }
}

internal final class ScanReadCancellation: @unchecked Sendable {
    private let lock = NSLock()
    private var cancelled = false
    func cancel() { lock.withLock { cancelled = true } }
    func check() throws {
        if lock.withLock({ cancelled }) { throw CancellationError() }
    }
}

private final class ScanReadRequest<Value: Sendable>: @unchecked Sendable {
    let cancellation = ScanReadCancellation()
    private let lock = NSLock()
    private var cancelled = false
    private var continuation: CheckedContinuation<Value, any Error>?

    func install(_ continuation: CheckedContinuation<Value, any Error>) -> Bool {
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
    func cancel() {
        cancellation.cancel()
        lock.lock()
        cancelled = true
        let pending = continuation
        continuation = nil
        lock.unlock()
        pending?.resume(throwing: CancellationError())
    }
    func finish(_ result: Result<Value, any Error>) {
        lock.lock()
        let pending = continuation
        continuation = nil
        lock.unlock()
        pending?.resume(with: result)
    }
}
