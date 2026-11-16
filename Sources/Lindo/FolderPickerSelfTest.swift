import Foundation
import LindoKit

/// Exercises the same request lifecycle used by AppModel without opening AppKit UI.
enum FolderPickerSelfTest {
    private final class Presenter: FolderPickerPresenting {
        var callbacks: [(URL?) -> Void] = []
        var cancellations = 0
        var available = true
        func begin(directory: URL, completion: @escaping (URL?) -> Void) -> Bool {
            guard available else { return false }
            callbacks.append(completion)
            return true
        }
        func cancel() { cancellations += 1 }
    }

    static func run() -> [(String, Bool)] {
        let presenter = Presenter()
        let coordinator = FolderPickerCoordinator(presenter: presenter)
        let initial = URL(fileURLWithPath: "/fixture/original")
        let chosen = URL(fileURLWithPath: "/fixture/chosen")
        var root = initial
        var scans = 0
        var applied = 0
        var page = SidebarItem.duplicates
        var eligible = true
        func begin(andScan: Bool) {
            let requestPage = page
            coordinator.begin(page: requestPage, directory: root, andScan: andScan,
                              canApply: { eligible && page == requestPage },
                              apply: { url, scan in
                root = url
                applied += 1
                if scan { scans += 1 }
            })
        }
        var results: [(String, Bool)] = []
        begin(andScan: true)
        results.append(("picker-call-returns-before-callback", coordinator.choosing && applied == 0 && presenter.callbacks.count == 1))
        begin(andScan: true)
        results.append(("picker-reentry-ignored", presenter.callbacks.count == 1))
        presenter.callbacks[0](nil)
        results.append(("picker-cancel-preserves-root-no-scan", !coordinator.choosing && root == initial && scans == 0 && applied == 0))
        begin(andScan: false)
        presenter.callbacks[1](chosen)
        results.append(("picker-reopens-and-choose-does-not-scan", root == chosen && applied == 1 && scans == 0 && !coordinator.choosing))
        begin(andScan: true)
        presenter.callbacks[2](initial)
        presenter.callbacks[2](chosen)
        results.append(("picker-confirm-scans-exactly-once", scans == 1 && applied == 2 && root == initial))
        begin(andScan: true)
        page = .largeFiles
        coordinator.cancel()
        begin(andScan: false)
        presenter.callbacks[3](chosen)
        let staleIgnored = coordinator.choosing && applied == 2 && scans == 1 && root == initial
        presenter.callbacks[4](chosen)
        results.append(("picker-page-change-cancels-stale-completion", staleIgnored && presenter.cancellations == 1 && !coordinator.choosing && applied == 3 && scans == 1))
        begin(andScan: true)
        eligible = false
        presenter.callbacks[5](initial)
        results.append(("picker-ineligible-completion-resets-state", !coordinator.choosing && root == chosen && applied == 3 && scans == 1))
        eligible = true
        presenter.available = false
        begin(andScan: true)
        results.append(("picker-missing-window-resets-state", !coordinator.choosing && applied == 3))
        return results
    }
}


extension FolderPickerSelfTest {
    @MainActor
    private final class LoadingHarness {
        var pending: [CheckedContinuation<FolderListing, Error>] = []
        func load(_ url: URL) async throws -> FolderListing {
            try await withCheckedThrowingContinuation { pending.append($0) }
        }
        func succeed(_ index: Int, directory: URL) {
            guard pending.indices.contains(index) else { return }
            pending[index].resume(returning: FolderListing(directory: directory, children: [],
                                                          warnings: [], canSelect: true, parent: nil))
        }
        func fail(_ index: Int, message: String) {
            guard pending.indices.contains(index) else { return }
            pending[index].resume(throwing: NSError(domain: "FolderBrowserSelfTest", code: index,
                                                   userInfo: [NSLocalizedDescriptionKey: message]))
        }
    }

    /// Controlled stalled reads require no filesystem access or AppKit UI.
    @MainActor
    static func runLoading() async -> [(String, Bool)] {
        let harness = LoadingHarness()
        let first = URL(fileURLWithPath: "/fixture/blocked")
        let second = URL(fileURLWithPath: "/fixture/retry")
        let model = FolderBrowserModel(initialURL: first, loadingTimeout: .milliseconds(50), loader: harness.load)
        var results: [(String, Bool)] = []
        let timedOutLoad = model.navigate(to: first)
        await waitUntil { !model.loading }
        results.append(("picker-stalled-load-times-out-visibly", !model.loading && model.error?.contains("troppo tempo") == true && !model.canSelect))
        let retryLoad = model.navigate(to: second)
        await waitUntil { harness.pending.count >= 2 }
        harness.fail(0, message: "stale response")
        await timedOutLoad.value
        results.append(("picker-late-load-cannot-overwrite-new-navigation", model.loading && model.error == nil && model.currentURL == second))
        harness.fail(1, message: "controlled retry completed")
        await retryLoad.value
        results.append(("picker-can-retry-after-load-timeout", !model.loading && model.currentURL == second && model.error?.contains("controlled retry completed") == true))
        let cancelledLoad = model.navigate(to: first)
        await waitUntil { harness.pending.count >= 3 }
        let started = ProcessInfo.processInfo.systemUptime
        model.stop()
        let elapsed = ProcessInfo.processInfo.systemUptime - started
        harness.fail(2, message: "cancelled response")
        await cancelledLoad.value
        results.append(("picker-cancel-stalled-load-is-prompt-and-discards-response", elapsed < 0.05 && !model.loading && model.error == nil && model.listing == nil))
        let editingHarness = LoadingHarness()
        let editingModel = FolderBrowserModel(initialURL: first, loader: editingHarness.load)
        let editedLoad = editingModel.navigate(to: first)
        await waitUntil { editingHarness.pending.count == 1 }
        editingModel.pathText = second.path(percentEncoded: false)
        editingHarness.succeed(0, directory: first)
        await editedLoad.value
        results.append(("picker-slow-load-preserves-unsubmitted-path",
                        editingModel.pathText == second.path(percentEncoded: false)
                        && editingModel.listing?.directory == first && !editingModel.loading
                        && editingModel.error == nil && !editingModel.canSelect))

        let invalidatedLoad = editingModel.navigate(to: first)
        await waitUntil { editingHarness.pending.count == 2 }
        editingModel.pathText = "relative/path"
        editingModel.navigatePath()
        let invalidError = editingModel.error
        let invalidState = !editingModel.loading && editingModel.listing == nil
            && invalidError?.contains("percorso completo") == true && !editingModel.canSelect
        editingHarness.succeed(1, directory: first)
        await invalidatedLoad.value
        results.append(("picker-invalid-submit-invalidates-pending-navigation",
                        invalidState && editingModel.pathText == "relative/path"
                        && editingModel.error == invalidError && editingModel.listing == nil
                        && !editingModel.loading && !editingModel.canSelect))
        return results
    }

    @MainActor
    private static func waitUntil(_ condition: () -> Bool) async {
        let deadline = ContinuousClock.now.advanced(by: .seconds(1))
        while !condition(), ContinuousClock.now < deadline {
            try? await Task.sleep(for: .milliseconds(1))
        }
    }
}
