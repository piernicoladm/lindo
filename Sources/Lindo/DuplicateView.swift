import AppKit
import LindoKit
import SwiftUI

struct DuplicateView: View {
    @Environment(AppModel.self) private var model
    @State private var showFailures = false
    @FocusState private var searchFocused: Bool

    var body: some View {
        @Bindable var model = model
        VStack(alignment: .leading, spacing: 12) {
            HStack(alignment: .top, spacing: 16) {
                VStack(alignment: .leading, spacing: 4) {
                    Text("Duplicati").font(.system(size: 20, weight: .semibold))
                    LastScanLabel(date: model.scanDates[.duplicates], identifier: "last-scan-duplicates")
                }
                Spacer(minLength: 0)
                if model.scanDates[.duplicates] != nil {
                    Button("Analizza di nuovo", systemImage: "magnifyingglass") { model.startScan() }
                        .buttonStyle(.bordered).controlSize(.regular)
                        .disabled(model.isBusy || model.isSample)
                        .accessibilityIdentifier("scan-duplicates")
                }
            }.gridProbe("duplicates-header", area: "duplicates")

            HStack(spacing: 12) {
                Image(systemName: "folder").foregroundStyle(.secondary)
                Text(model.duplicateRoot?.path(percentEncoded: false) ?? "Scegli una cartella locale")
                        .font(.system(size: 13)).lineLimit(1).truncationMode(.middle)
                        .help(model.duplicateRoot?.path ?? "Scegli la cartella da analizzare")
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .drawingGroup()
                Picker("Dimensione minima", selection: $model.duplicateMinimumKB) {
                    Text("Tutti").tag(0)
                    Text("1 KB+").tag(1)
                    Text("1 MB+").tag(1000)
                    Text("100 MB+").tag(100_000)
                }.frame(width: 240).disabled(model.isBusy || model.isSample)
                Button("Scegli…") { model.chooseFolder() }.disabled(model.isBusy || model.isSample)
            }.padding(.vertical, 8)

            if !(model.warnings[.duplicates] ?? []).isEmpty {
                Button { model.showWarnings = true } label: {
                    Label("Analisi parziale o elementi esclusi. Mostra dettagli", systemImage: "exclamationmark.triangle")
                        .font(.callout).frame(maxWidth: .infinity, alignment: .leading).padding(.vertical, 4)
                }.buttonStyle(.plain).foregroundStyle(.orange)
            }
            if let notice = model.notice {
                VStack(alignment: .leading, spacing: 4) {
                    Label(notice, systemImage: model.failures.isEmpty ? "checkmark.circle" : "info.circle").font(.callout)
                    if !model.failures.isEmpty { Button("Mostra dettagli degli elementi non spostati") { showFailures = true }.font(.caption) }
                }.padding(.vertical, 4).frame(maxWidth: .infinity, alignment: .leading)
            }

            if model.isScanningCurrentPage && model.duplicateGroups.isEmpty {
                emptyMessage("Confronto dei file in corso", detail: "I gruppi appariranno al termine del confronto. Puoi cambiare sezione mentre Lindo lavora.")
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else if model.scanDates[.duplicates] == nil {
                VStack(spacing: 12) {
                    emptyMessage("Trova copie identiche", detail: "Scegli una cartella. Lindo confronta il contenuto dei file locali e protegge sempre una copia di ogni gruppo.")
                    Button(model.duplicateRoot == nil ? "Scegli e analizza" : "Avvia analisi") { model.startScan() }.buttonStyle(.borderedProminent).controlSize(.regular).disabled(model.isSample || model.isBusy).accessibilityIdentifier("scan-duplicates")
                }.frame(maxWidth: .infinity, maxHeight: .infinity)
            } else if model.duplicateGroups.isEmpty {
                emptyMessage(model.emptyScanMessage(for: .duplicates).title, detail: model.emptyScanMessage(for: .duplicates).detail)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                groupList
            }

            if !model.duplicateGroups.isEmpty {
                SelectionFooter(count: model.selectedItems.count, size: model.selectedBytes,
                    note: "Una copia per gruppo resta protetta. Dimensioni logiche, spazio recuperato variabile.",
                    disabled: model.isBusy, identifier: "review-duplicates", hiddenCount: model.hiddenSelectedCount,
                    clearFilter: { model.clearSearch() }) { model.reviewSelection() }
            }
        }
        .padding(16).font(.system(size: 13)).controlSize(.small)
        .gridArea("duplicates", inset: 16)
        .onChange(of: model.searchFocusRequest) { _, _ in
            if model.sidebar == .duplicates && !model.duplicateGroups.isEmpty { searchFocused = true }
        }
        .onChange(of: model.duplicateMinimumKB) { _, _ in model.clearResults(.duplicates) }
        .sheet(isPresented: $showFailures) { DetailsSheet(title: "Elementi non spostati", lines: model.failures) }
    }

    private var groupList: some View {
        @Bindable var model = model
        return VStack(spacing: 0) {
            VStack(alignment: .leading, spacing: 12) {
                HStack(spacing: 8) {
                    VStack(alignment: .leading, spacing: 4) {
                        Text("\(model.duplicateGroups.count) \(model.duplicateGroups.count == 1 ? "gruppo" : "gruppi") · \(bytes(model.duplicateExtraBytes)) extra")
                            .font(.callout.weight(.medium))
                            .help("Dimensione delle copie extra, esclusa la copia protetta di ogni gruppo")
                        if !model.query.isEmpty {
                            Text(model.matchingDuplicateGroups.count == 1 ? "1 gruppo visibile" : "\(model.matchingDuplicateGroups.count) gruppi visibili")
                                .font(.caption).foregroundStyle(.secondary)
                        }
                    }.frame(maxWidth: .infinity, alignment: .leading)
                    Button(model.query.isEmpty ? "Seleziona extra" : "Seleziona extra visibili") { model.selectDuplicateCopies() }
                        .disabled(model.isBusy || model.matchingDuplicateGroups.isEmpty)
                        .accessibilityIdentifier("select-extra-copies")
                        .help(model.query.isEmpty ? "Seleziona le copie extra di tutti i gruppi" : "Seleziona le copie extra dei gruppi visibili")
                    if !model.selected.isEmpty {
                        Button("Deseleziona tutto") { model.selected.removeAll() }
                            .disabled(model.isBusy).accessibilityIdentifier("deselect-duplicates")
                    }
                    ResultsSearchField(query: $model.query, prompt: "Cerca nei gruppi", identifier: "search-duplicates", focused: $searchFocused) { model.clearSearch() }
                }.font(.caption)
                HStack(spacing: 12) {
                    Picker("Ordina gruppi", selection: $model.duplicateSortOrder) {
                        ForEach(DuplicateSortOrder.allCases) { order in
                            Text(order.title).tag(order)
                        }
                    }
                    .pickerStyle(.menu).frame(width: 240)
                    .disabled(model.isBusy || model.showConfirmation)
                    .accessibilityIdentifier("duplicate-sort-order")
                    Text(model.duplicateSortOrder == .extraBytes
                         ? "Prima i gruppi con più byte logici nelle copie extra."
                         : "Ordine del primo percorso di ogni gruppo.")
                        .font(.caption).foregroundStyle(.secondary)
                        .lineLimit(2)
                    Spacer(minLength: 0)
                }
                Text("La copia protetta iniziale segue l’ordine dei percorsi. Puoi cambiarla: Lindo non identifica un originale.")
                    .font(.caption).foregroundStyle(.secondary)
                    .lineLimit(2)
                if model.isScanningCurrentPage {
                    Text("Risultati dell’analisi precedente. Il nuovo confronto è in corso.")
                        .font(.caption).foregroundStyle(.secondary)
                }
            }.padding(.vertical, 8)
            Divider()
            if model.matchingDuplicateGroups.isEmpty {
                emptyMessage("Nessun risultato per questo filtro", detail: "Modifica la ricerca o cancellala per mostrare tutti i risultati. Le selezioni restano conservate.")
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 0) {
                        ForEach(model.resultsProjection.rows) { row in
                            switch row {
                            case .header(let group):
                                HStack {
                                    Label("\(group.files.count) copie identiche", systemImage: "doc.on.doc")
                                        .font(.callout.weight(.medium)).foregroundStyle(.secondary)
                                    Spacer()
                                    Text("\(bytes(group.files.first?.bytes ?? 0)) ciascuna").font(.caption).foregroundStyle(.secondary)
                                }.padding(8)
                                .background(Color(nsColor: .controlBackgroundColor))
                            case .file(let file, let group):
                                VStack(spacing: 0) {
                                    duplicateRow(file, group: group)
                                    Divider().padding(.leading, 36)
                                }
                            }
                        }
                    }
                }
                .frame(minHeight: 0, maxHeight: .infinity)
                .scrollBounceBehavior(.basedOnSize)
            }
        }
        // Let the list shrink within the page so the selection footer remains visible.
        .frame(maxWidth: .infinity, minHeight: 0, maxHeight: .infinity)
        .background(Color(nsColor: .textBackgroundColor))
    }

    private func duplicateRow(_ file: ScanItem, group: DuplicateGroup) -> some View {
        let keeper = model.keeperID(for: group) == file.id
        return HStack(spacing: 12) {
            if keeper {
                Image(systemName: "lock.fill").foregroundStyle(.secondary)
                    .frame(width: 20).accessibilityLabel("Copia protetta: \(file.url.path)")
            } else {
                Toggle(isOn: Binding(get: { model.selected.contains(file.id) }, set: { _ in model.toggle(file) })) { Text("Seleziona copia \(file.title)") }
                    .labelsHidden().toggleStyle(.checkbox).disabled(model.isBusy)
                    .accessibilityLabel("Seleziona copia: \(file.url.path)")
                    .frame(width: 20)
            }
            VStack(alignment: .leading, spacing: 4) {
                Text(file.title).font(.system(size: 13, weight: .medium)).lineLimit(1).truncationMode(.middle)
                Text(file.url.path).font(.system(size: 12)).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle)
                    .help(file.url.path)
            }
            Spacer(minLength: 0)
            if keeper {
                Text("Copia protetta").font(.system(size: 12, weight: .medium)).foregroundStyle(.primary)
                    .help("Questa copia resta esclusa dalla selezione per il Cestino.")
            } else {
                Button("Conserva questa") { model.keep(file, in: group) }
                    .font(.caption).buttonStyle(.borderless).disabled(model.isBusy)
                    .accessibilityLabel("Conserva questa copia: \(file.url.path)")
            }
            Button { NSWorkspace.shared.activateFileViewerSelecting([file.url]) } label: { Image(systemName: "folder") }
                .buttonStyle(.borderless).help("Mostra nel Finder").accessibilityLabel("Mostra nel Finder: \(file.url.path)").disabled(model.isSample)
        }
        .padding(.horizontal, 8).padding(.vertical, 4)
        .onAppear { if model.isSample { model.appearedDuplicateRows.insert(file.id) } }
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
