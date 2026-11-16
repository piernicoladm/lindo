import Foundation
import Observation
import SwiftUI

protocol FolderPickerPresenting: AnyObject {
    @discardableResult
    func begin(directory: URL, completion: @escaping (URL?) -> Void) -> Bool
    func cancel()
}

/// Owns a request until its asynchronous completion, including cancellation races.
@Observable
final class FolderPickerCoordinator {
    private(set) var choosing = false
    private var requestID: UUID?
    private let presenter: any FolderPickerPresenting

    init(presenter: any FolderPickerPresenting) { self.presenter = presenter }

    func begin(page: SidebarItem, directory: URL, andScan: Bool,
               canApply: @escaping () -> Bool,
               apply: @escaping (URL, Bool) -> Void) {
        guard !choosing else { return }
        let id = UUID()
        requestID = id
        choosing = true
        let presented = presenter.begin(directory: directory) { [weak self] url in
            guard let self, self.requestID == id else { return }
            self.requestID = nil
            self.choosing = false
            guard let url, canApply() else { return }
            apply(url, andScan)
        }
        if !presented, requestID == id {
            requestID = nil
            choosing = false
        }
    }

    func cancel() {
        guard choosing else { return }
        requestID = nil
        choosing = false
        presenter.cancel()
    }
}

/// Presents in the existing SwiftUI window, without invoking synchronous AppKit sheet setup.
@Observable
final class InlineFolderPicker: FolderPickerPresenting {
    private(set) var browser: FolderBrowserModel?
    private var completion: ((URL?) -> Void)?

    func begin(directory: URL, completion: @escaping (URL?) -> Void) -> Bool {
        guard browser == nil else { return false }
        let timing = PickerTiming()
        self.completion = completion
        browser = FolderBrowserModel(initialURL: directory)
        timing.mark("model-presented")
        return true
    }

    func cancel() { finish(nil) }

    func select(_ url: URL) {
        guard let browser, browser.canSelect, let listing = browser.listing,
              url.standardizedFileURL == listing.directory.standardizedFileURL else { return }
        finish(listing.directory)
    }

    private func finish(_ url: URL?) {
        guard let browser else { return }
        browser.stop()
        self.browser = nil
        let callback = completion
        completion = nil
        callback?(url)
    }
}

struct FolderPickerOverlay: ViewModifier {
    let model: AppModel

    func body(content: Content) -> some View {
        content
            .disabled(model.choosingFolder)
            .accessibilityHidden(model.choosingFolder)
            .overlay {
                if let browser = model.presentedFolderBrowser {
                    ZStack {
                        Color.black.opacity(0.28).ignoresSafeArea()
                            .contentShape(Rectangle())
                            .onTapGesture { }
                        FolderBrowserView(model: browser, onCancel: model.cancelFolderChoice,
                                          onSelect: model.completeFolderChoice)
                            .frame(width: 660, height: 500)
                            .background(Palette.surface, in: RoundedRectangle(cornerRadius: 16))
                            .clipShape(RoundedRectangle(cornerRadius: 16))
                            .shadow(color: .black.opacity(0.22), radius: 24, y: 8)
                            .accessibilityElement(children: .contain)
                    }
                    .disabled(false)
                }
            }
    }
}

/// Opt-in stage measurements. Never logs selected paths or user content.
private final class PickerTiming {
    private let enabled = ProcessInfo.processInfo.environment["LINDO_PICKER_TIMING"] == "1"
    private let started = ProcessInfo.processInfo.systemUptime

    func mark(_ stage: String) {
        guard enabled else { return }
        let now = ProcessInfo.processInfo.systemUptime
        let line = String(format: "PICKER_TIMING stage=%@ total_ms=%.2f\n", stage, (now - started) * 1000)
        FileHandle.standardError.write(Data(line.utf8))
    }
}
