import AppKit
import LindoKit
import SwiftUI

struct ResultsView: View {
    @Environment(AppModel.self) private var model
    @State private var showFailures = false
    @FocusState private var searchFocused: Bool

    var body: some View {
        @Bindable var model = model
        VStack(alignment: .leading, spacing: 12) {
            HStack(alignment: .top, spacing: 16) {
                VStack(alignment: .leading, spacing: 4) {
                    Text(model.sidebar.title).font(.system(size: 20, weight: .semibold))
                    LastScanLabel(date: model.scanDates[model.sidebar], identifier: "last-scan-results")
                }
                Spacer(minLength: 0)
                if model.scanDates[model.sidebar] != nil {
                    Button("Analizza di nuovo", systemImage: "magnifyingglass") { model.startScan() }
                        .buttonStyle(.bordered).controlSize(.regular)
                        .disabled(model.isBusy || model.isSample)
                        .accessibilityIdentifier("scan")
                }
            }
            .gridProbe("results-header", area: "results")

            if model.sidebar == .largeFiles {
                HStack(spacing: 12) {
                    Image(systemName: "folder").foregroundStyle(.secondary)
                    Text(model.largeRoot?.path(percentEncoded: false) ?? "Scegli una cartella locale")
                        .font(.system(size: 13)).lineLimit(1).truncationMode(.middle)
                        .help(model.largeRoot?.path ?? "Scegli la cartella da analizzare")
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .drawingGroup()
                    Picker("Dimensione minima", selection: $model.minimumMB) {
                        Text("100 MB+").tag(100)
                        Text("500 MB+").tag(500)
                        Text("1 GB+").tag(1000)
                    }.frame(width: 240).disabled(model.isBusy || model.isSample)
                    Button("Scegli…") { model.chooseFolder() }.disabled(model.isBusy || model.isSample)
                }
                .padding(.vertical, 8)
            }

            if model.sidebar == .cleanup {
                Label("Cache web di Chrome, Chromium e Firefox: chiudi i browser prima di pulire. Le altre cache restano escluse.", systemImage: "checkmark.shield")
                    .font(.caption).foregroundStyle(.secondary)
            }

            if let messages = model.warnings[model.sidebar], !messages.isEmpty {
                Button { model.showWarnings = true } label: {
                    HStack(spacing: 8) {
                        Image(systemName: model.scanOutcome(for: model.sidebar) == .excluded ? "info.circle" : "exclamationmark.triangle")
                        Text(model.coverageNotice(for: model.sidebar)).font(.callout)
                        Spacer()
                        Text("Dettagli (\(messages.count))").font(.caption.weight(.medium))
                        Image(systemName: "chevron.right").font(.caption)
                    }.padding(.vertical, 4)
                }
                .buttonStyle(.plain)
                .foregroundStyle(model.scanOutcome(for: model.sidebar) == .excluded ? Color.secondary : Color.orange)
            }

            if let notice = model.notice {
                VStack(alignment: .leading, spacing: 4) {
                    Label(notice, systemImage: model.failures.isEmpty ? "checkmark.circle" : "info.circle").font(.callout)
                    if !model.failures.isEmpty {
                        Button("Mostra \(model.failures.count) elementi non spostati") { showFailures = true }.font(.caption)
                    }
                }.padding(.vertical, 4).frame(maxWidth: .infinity, alignment: .leading)
            }

            if model.isScanningCurrentPage && model.currentItems.isEmpty {
                emptyMessage("Analisi in corso", detail: "I risultati appariranno al termine. Puoi cambiare sezione mentre Lindo lavora.")
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else if model.scanDates[model.sidebar] == nil {
                emptyState
            } else if model.currentItems.isEmpty {
                emptyMessage(model.emptyScanMessage(for: model.sidebar).title, detail: model.emptyScanMessage(for: model.sidebar).detail)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                resultsList
            }

            if !model.currentItems.isEmpty {
                SelectionFooter(count: model.selectedItems.count, size: model.selectedBytes,
                    note: model.sidebar == .applications ? "Solo il bundle .app. I dati personali restano al loro posto." : "Potrai ripristinare i file dal Cestino finché non lo svuoti.",
                    disabled: model.isBusy, identifier: "review-trash", hiddenCount: model.hiddenSelectedCount,
                    clearFilter: { model.clearSearch() }) { model.reviewSelection() }
            }
        }
        .padding(16)
        .font(.system(size: 13))
        .controlSize(.small)
        .gridArea("results", inset: 16)
        .onChange(of: model.searchFocusRequest) { _, _ in
            if [.cleanup, .largeFiles, .applications].contains(model.sidebar) && !model.currentItems.isEmpty { searchFocused = true }
        }
        .sheet(isPresented: $showFailures) { DetailsSheet(title: "Elementi non spostati", lines: model.failures) }
    }

    private var emptyState: some View {
        VStack(spacing: 12) {
            Text(model.sidebar == .cleanup ? "Analizza cache web e log" : model.sidebar == .largeFiles ? "Trova i file grandi" : "Esamina le applicazioni installate")
                .font(.system(size: 13, weight: .semibold))
            Text(model.sidebar == .cleanup ? "Analizza le cache web riconosciute e i vecchi log.\nLe cache vengono trattate come cartelle complete." : model.sidebar == .largeFiles ? "Scegli una cartella: Lindo legge dimensioni e date,\nsenza aprire i documenti o scaricare file dal cloud." : "Analizza le cartelle Applicazioni. Le app in esecuzione\ne quelle di sistema vengono protette dalla rimozione.")
                .foregroundStyle(.secondary).multilineTextAlignment(.center)
            Button(model.sidebar == .largeFiles && model.largeRoot == nil ? "Scegli e analizza" : "Avvia analisi") { model.startScan() }
                .buttonStyle(.borderedProminent).controlSize(.regular)
                .disabled(model.isBusy || model.isSample)
                .accessibilityIdentifier("scan")
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private var resultsList: some View {
        @Bindable var model = model
        return VStack(spacing: 0) {
            VStack(alignment: .leading, spacing: 12) {
                HStack(spacing: 8) {
                    VStack(alignment: .leading, spacing: 4) {
                        Text("\(elementCount(model.currentItems.count)) · \(bytes(model.listedBytes))")
                            .font(.callout.weight(.medium))
                        if !model.query.isEmpty {
                            Text("\(model.visibleItems.count) visibili")
                                .font(.caption).foregroundStyle(.secondary)
                        }
                    }.frame(maxWidth: .infinity, alignment: .leading)
                    if model.sidebar != .applications {
                        Button("Seleziona visibili") { model.selected.formUnion(model.visibleItems.map(\.id)) }
                            .disabled(model.isBusy || model.visibleItems.isEmpty)
                    } else {
                        Text("Seleziona le app singolarmente").foregroundStyle(.secondary)
                    }
                    if !model.selected.isEmpty {
                        Button("Deseleziona tutto") { model.selected.removeAll() }.disabled(model.isBusy)
                    }
                    ResultsSearchField(query: $model.query, prompt: "Cerca nei risultati", identifier: "search-results", focused: $searchFocused) { model.clearSearch() }
                }.font(.caption)
                if model.isScanningCurrentPage {
                    Text("Risultati dell’analisi precedente. L’aggiornamento è in corso.")
                        .font(.caption).foregroundStyle(.secondary)
                }
            }.padding(.vertical, 8)
            Divider()
            if model.visibleItems.isEmpty {
                emptyMessage("Nessun risultato per questo filtro", detail: "Modifica la ricerca o cancellala per mostrare tutti i risultati. Le selezioni restano conservate.")
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                ScrollView {
                    LazyVStack(spacing: 0) {
                        ForEach(model.visibleItems) { item in
                            resultRow(item)
                            Divider().padding(.leading, 40)
                        }
                    }
                }
                .scrollBounceBehavior(.basedOnSize)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Color(nsColor: .textBackgroundColor))
    }

    private func resultRow(_ item: ScanItem) -> some View {
        HStack(spacing: 12) {
            Toggle(isOn: Binding(get: { model.selected.contains(item.id) }, set: { _ in model.toggle(item) })) { Text("Seleziona \(item.title)") }
                .labelsHidden().toggleStyle(.checkbox).disabled(model.isBusy)
                .accessibilityLabel("Seleziona \(item.title)")
            Image(systemName: item.category == .applications ? "macwindow" : item.category == .logs ? "doc.text" : item.category == .caches ? "archivebox" : "doc")
                .font(.system(size: 16)).foregroundStyle(.secondary)
                .frame(width: 20)
            VStack(alignment: .leading, spacing: 4) {
                Text(item.title).font(.system(size: 13, weight: .medium)).lineLimit(1).truncationMode(.middle)
                Text(item.url.path).font(.system(size: 12)).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle)
                Text("\(item.detail) · \(item.modifiedAt.formatted(date: .abbreviated, time: .omitted))")
                    .font(.system(size: 12)).foregroundStyle(.secondary).lineLimit(1)
            }
            Spacer(minLength: 8)
            Text(bytes(item.bytes)).font(.callout.weight(.medium)).monospacedDigit()
                .fixedSize()
            Button { NSWorkspace.shared.activateFileViewerSelecting([item.url]) } label: {
                Image(systemName: "folder").frame(width: 20, height: 20)
            }
            .buttonStyle(.borderless).help("Mostra nel Finder").accessibilityLabel("Mostra \(item.title) nel Finder")
            .disabled(model.isSample)
        }
        .padding(.horizontal, 8).padding(.vertical, 4)
        .contentShape(Rectangle())
        .help(item.url.path)
    }

    private func emptyMessage(_ title: String, detail: String) -> some View {
        VStack(spacing: 8) {
            Text(title).font(.system(size: 13, weight: .semibold))
            Text(detail).foregroundStyle(.secondary).multilineTextAlignment(.center)
                .frame(maxWidth: 380)
        }
        .padding(16)
    }
}
