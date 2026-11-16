import Foundation
import LindoKit
import Observation
import SwiftUI

/// Reads directory contents through the dedicated actor; published state stays on the UI actor.
@MainActor
@Observable
final class FolderBrowserModel {
    let initialURL: URL
    private(set) var currentURL: URL
    var pathText: String {
        didSet { pathRevision &+= 1 }
    }
    private(set) var loading = false
    private(set) var error: String?
    private(set) var listing: FolderListing?
    @ObservationIgnored private let loader: (URL) async throws -> FolderListing
    @ObservationIgnored private let loadingTimeout: Duration
    @ObservationIgnored private var timeoutTask: Task<Void, Never>?
    @ObservationIgnored private var loadTask: Task<Void, Never>?
    @ObservationIgnored private var requestID: UUID?
    @ObservationIgnored private var pathRevision: UInt64 = 0

    init(initialURL: URL, loadingTimeout: Duration = .seconds(5),
         loader: ((URL) async throws -> FolderListing)? = nil) {
        let reader = FolderDirectoryReader()
        self.loader = loader ?? { url in try await reader.list(url) }
        self.loadingTimeout = loadingTimeout
        self.initialURL = initialURL
        currentURL = initialURL
        pathText = initialURL.path(percentEncoded: false)
    }

    var canSelect: Bool {
        let expanded = (pathText as NSString).expandingTildeInPath
        guard expanded.hasPrefix("/") else { return false }
        let enteredURL = URL(fileURLWithPath: expanded, isDirectory: true).standardizedFileURL
        return !loading && error == nil && listing?.canSelect == true
            && listing?.directory == currentURL && enteredURL == currentURL.standardizedFileURL
    }

    /// The returned task completes after the model has consumed or discarded the response.
    @discardableResult
    func navigate(to url: URL) -> Task<Void, Never> {
        timeoutTask?.cancel()
        timeoutTask = nil
        loadTask?.cancel()
        let id = UUID()
        requestID = id
        currentURL = url
        pathText = url.path(percentEncoded: false)
        let submittedPathRevision = pathRevision
        listing = nil
        error = nil
        loading = true
        timeoutTask = Task { [weak self, loadingTimeout] in
            do { try await Task.sleep(for: loadingTimeout) }
            catch { return }
            guard let self, self.requestID == id, self.loading else { return }
            self.requestID = nil
            self.loadTask?.cancel()
            self.loadTask = nil
            self.timeoutTask = nil
            self.loading = false
            self.error = "Il caricamento richiede troppo tempo. Potrebbe esserci una richiesta di accesso di macOS o un’attesa del servizio cloud. Controlla gli avvisi, scegli una cartella locale o premi Vai per riprovare."
        }
        let task = Task {
            do {
                let result = try await loader(url)
                guard !Task.isCancelled, requestID == id else { return }
                listing = result
                currentURL = result.directory
                if pathRevision == submittedPathRevision {
                    pathText = result.directory.path(percentEncoded: false)
                }
                timeoutTask?.cancel()
                timeoutTask = nil
                requestID = nil
                loading = false
                loadTask = nil
            } catch {
                guard !Task.isCancelled, requestID == id else { return }
                self.error = "Impossibile aprire la cartella: \(error.localizedDescription)"
                timeoutTask?.cancel()
                timeoutTask = nil
                requestID = nil
                loading = false
                loadTask = nil
            }
        }
        loadTask = task
        return task
    }

    func navigatePath() {
        let expanded = (pathText as NSString).expandingTildeInPath
        guard expanded.hasPrefix("/") else {
            stop()
            listing = nil
            error = "Inserisci un percorso completo, per esempio ~/Downloads."
            return
        }
        navigate(to: URL(fileURLWithPath: expanded, isDirectory: true))
    }

    func stop() {
        timeoutTask?.cancel()
        timeoutTask = nil
        requestID = nil
        loadTask?.cancel()
        loadTask = nil
        loading = false
    }
}

@MainActor
struct FolderBrowserView: View {
    @Bindable var model: FolderBrowserModel
    let onCancel: () -> Void
    let onSelect: (URL) -> Void

    private var favorites: [(String, String, URL)] {
        let home = FileManager.default.homeDirectoryForCurrentUser
        return [("Inizio", "house", home),
                ("Download", "arrow.down.circle", home.appending(path: "Downloads")),
                ("Documenti", "doc", home.appending(path: "Documents")),
                ("Scrivania", "desktopcomputer", home.appending(path: "Desktop")),
                ("Cloud", "cloud", home.appending(path: "Library/CloudStorage"))]
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack(spacing: 12) {
                Image(systemName: "folder").font(.system(size: 20)).foregroundStyle(Palette.accentText)
                    .frame(width: 40, height: 40)
                    .background(Palette.accent.opacity(0.08), in: RoundedRectangle(cornerRadius: 12))
                VStack(alignment: .leading, spacing: 4) {
                    Text("Scegli una cartella").font(.system(size: 20, weight: .semibold))
                    Text("Verranno analizzati solo i file locali nella cartella scelta.")
                        .font(.caption).foregroundStyle(.secondary)
                }
            }
            HStack(spacing: 8) {
                Button { if let parent = model.listing?.parent { model.navigate(to: parent) } } label: {
                    Image(systemName: "arrow.up")
                }
                .help("Cartella superiore")
                .disabled(model.listing?.parent == nil)
                TextField("Percorso della cartella", text: $model.pathText)
                    .textFieldStyle(.roundedBorder)
                    .onSubmit { model.navigatePath() }
                    .accessibilityIdentifier("folder-browser-path")
                    .accessibilityLabel("Percorso della cartella")
                Button("Vai") { model.navigatePath() }
            }
            HStack(spacing: 0) {
                VStack(alignment: .leading, spacing: 8) {
                    ForEach(favorites, id: \.0) { name, symbol, url in
                        Button { model.navigate(to: url) } label: {
                            Label(name, systemImage: symbol).frame(maxWidth: .infinity, alignment: .leading).padding(.vertical, 8)
                        }.buttonStyle(.plain)
                    }
                    Spacer()
                }.padding(12).frame(width: 144)
                Divider()
                ZStack {
                    List(model.listing?.children ?? []) { entry in
                        Button { model.navigate(to: entry.url) } label: {
                            HStack {
                                Image(systemName: "folder").foregroundStyle(.secondary)
                                Text(entry.name).lineLimit(1)
                                Spacer()
                                Image(systemName: "chevron.right").font(.caption).foregroundStyle(.secondary)
                            }.padding(.vertical, 4).contentShape(Rectangle())
                        }.buttonStyle(.plain)
                    }.listStyle(.inset)
                    if model.loading {
                        ProgressView("Caricamento cartelle…").padding(20).background(.regularMaterial, in: RoundedRectangle(cornerRadius: 12))
                    } else if model.listing?.children.isEmpty == true {
                        Text("Nessuna sottocartella disponibile.").foregroundStyle(.secondary)
                    }
                }
            }
            .background(Color(nsColor: .controlBackgroundColor))
            .clipShape(RoundedRectangle(cornerRadius: 8))
            if let error = model.error {
                Text(error).font(.caption).foregroundStyle(.red).lineLimit(3)
            } else if let warnings = model.listing?.warnings, !warnings.isEmpty {
                Text(warnings.joined(separator: " ")).font(.caption).foregroundStyle(.secondary).lineLimit(2)
            }
            HStack {
                Text(model.currentURL.lastPathComponent.isEmpty ? "/" : model.currentURL.lastPathComponent)
                    .font(.caption).foregroundStyle(.secondary).lineLimit(1)
                Spacer()
                Button("Annulla", action: onCancel).keyboardShortcut(.cancelAction)
                Button("Seleziona questa cartella") {
                    guard model.canSelect, let listing = model.listing else { return }
                    onSelect(listing.directory)
                }
                .buttonStyle(.borderedProminent)
                .disabled(!model.canSelect)
                .accessibilityIdentifier("folder-browser-select")
            }
        }
        .padding(20)
        .frame(minWidth: 620, minHeight: 460)
        .gridArea("folder-browser", inset: 20)
        .task { model.navigate(to: model.initialURL) }
        .onDisappear { model.stop() }
    }
}
